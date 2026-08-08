//==============================================================================
// elf.rs — ELF64 loader
//
// Parses ELF64 executables, loads PT_LOAD segments into memory, and returns
// the entry point.
//==============================================================================

// ELF loader — wired into sys_exec() for ELF64 binary support

use crate::paging;
use crate::pmm::PmmAllocator;
use core::fmt::Write;

/// Guard that restores interrupt flag on drop.
/// Save the IF bit before CLI, then restore (STI) when the guard drops
/// — even on early returns (panics, errors, etc.).
struct IrqGuard(u64);

impl Drop for IrqGuard {
    fn drop(&mut self) {
        if self.0 & 0x200 != 0 {
            unsafe {
                core::arch::asm!("sti", options(nostack));
            }
        }
    }
}

//--- ELF64 constants ----------------------------------------------------------

/// ELF magic number (first 4 bytes of a valid ELF file).
const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];

/// ELF class: 64-bit.
const ELFCLASS64: u8 = 2;

/// ELF little-endian.
const ELFDATA2LSB: u8 = 1;

/// ET_EXEC — executable type.
const ET_EXEC: u16 = 2;

/// PT_LOAD — loadable segment type.
const PT_LOAD: u32 = 1;

/// x86-64 architecture.
const EM_X86_64: u16 = 0x3E;

//--- ELF64 header structures --------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy)]
struct Elf64Ehdr {
    e_ident: [u8; 16], // ELF identification
    e_type: u16,       // Object file type
    e_machine: u16,    // Architecture
    e_version: u32,    // Object file version
    e_entry: u64,      // Entry point virtual address
    e_phoff: u64,      // Program header offset
    e_shoff: u64,      // Section header offset
    e_flags: u32,      // Processor-specific flags
    e_ehsize: u16,     // ELF header size
    e_phentsize: u16,  // Size of program header entry
    e_phnum: u16,      // Number of program header entries
    e_shentsize: u16,  // Size of section header entry
    e_shnum: u16,      // Number of section header entries
    e_shstrndx: u16,   // Section header string table index
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Elf64Phdr {
    p_type: u32,   // Segment type
    p_flags: u32,  // Segment flags
    p_offset: u64, // Segment file offset
    p_vaddr: u64,  // Segment virtual address
    p_paddr: u64,  // Segment physical address
    p_filesz: u64, // Segment size in file
    p_memsz: u64,  // Segment size in memory
    p_align: u64,  // Segment alignment
}

//--- Loader -------------------------------------------------------------------

/// Errors that can occur during ELF loading.
#[derive(Debug)]
pub enum ElfError {
    BadMagic,
    BadClass,
    BadEndian,
    BadMachine,
    BadType,
    Truncated,
    Oom,
    MetadataOom,
}

/// Provenance for a page allocated by one ELF load operation.
#[derive(Clone, Copy)]
struct LoadedPage {
    vaddr: u64,
    paddr: u64,
}

const PROVENANCE_CHUNK_CAP: usize = 64;

#[repr(C)]
#[derive(Clone, Copy)]
struct LoadedPageChunk {
    next: *mut LoadedPageChunk,
    used: usize,
    pages: [LoadedPage; PROVENANCE_CHUNK_CAP],
}

// Metadata is kept in a kernel-owned static arena rather than the general
// heap.  The loader runs while changing CR3 and its cleanup must not depend on
// kmm free-list state.  This supports 4096 tracked pages per load and returns
// every chunk to the arena on success or failure.
const PROVENANCE_CHUNK_COUNT: usize = 64;
static mut PROVENANCE_POOL: [LoadedPageChunk; PROVENANCE_CHUNK_COUNT] = [LoadedPageChunk {
    next: core::ptr::null_mut(),
    used: 0,
    pages: [LoadedPage { vaddr: 0, paddr: 0 }; PROVENANCE_CHUNK_CAP],
}; PROVENANCE_CHUNK_COUNT];
static mut PROVENANCE_FREE_MASK: u64 = u64::MAX;

struct LoadedPages {
    head: *mut LoadedPageChunk,
    tail: *mut LoadedPageChunk,
    len: usize,
}

impl LoadedPages {
    const fn new() -> Self {
        Self {
            head: core::ptr::null_mut(),
            tail: core::ptr::null_mut(),
            len: 0,
        }
    }

    /// Reserve a slot before allocating or mapping its physical page.
    fn reserve(&mut self) -> Result<*mut LoadedPage, ElfError> {
        unsafe {
            if self.tail.is_null() || (*self.tail).used == PROVENANCE_CHUNK_CAP {
                let free_mask = PROVENANCE_FREE_MASK;
                if free_mask == 0 {
                    return Err(ElfError::MetadataOom);
                }
                let index = free_mask.trailing_zeros() as usize;
                PROVENANCE_FREE_MASK &= !(1u64 << index);
                let chunk = core::ptr::addr_of_mut!(PROVENANCE_POOL[index]);
                (*chunk).next = core::ptr::null_mut();
                (*chunk).used = 0;
                if self.tail.is_null() {
                    self.head = chunk;
                } else {
                    (*self.tail).next = chunk;
                }
                self.tail = chunk;
            }
            let slot = &mut (*self.tail).pages[(*self.tail).used] as *mut LoadedPage;
            (*self.tail).used += 1;
            self.len += 1;
            Ok(slot)
        }
    }

    fn cancel_reservation(&mut self) {
        unsafe {
            if !self.tail.is_null() && (*self.tail).used > 0 {
                (*self.tail).used -= 1;
                self.len -= 1;
            }
        }
    }

    fn find(&self, vaddr: u64) -> Option<u64> {
        unsafe {
            let mut chunk = self.head;
            while !chunk.is_null() {
                for page in &(&(*chunk).pages)[..(*chunk).used] {
                    if page.vaddr == vaddr {
                        return Some(page.paddr);
                    }
                }
                chunk = (*chunk).next;
            }
        }
        None
    }

    unsafe fn rollback(&mut self, pmm: &mut PmmAllocator, pml4_phys: u64) {
        let saved_cr3 = paging::read_cr3();
        if saved_cr3 != pml4_phys {
            unsafe {
                paging::write_cr3(pml4_phys);
            }
        }
        let mut chunk = self.head;
        while !chunk.is_null() {
            for page in &(&(*chunk).pages)[..(*chunk).used] {
                if let Some(entry) = paging::unmap(page.vaddr) {
                    pmm.free((entry & 0x000F_FFFF_FFFF_F000u64) as *mut u8);
                }
            }
            chunk = (*chunk).next;
        }
        if saved_cr3 != pml4_phys {
            unsafe {
                paging::write_cr3(saved_cr3);
            }
        }
    }

    unsafe fn release_metadata(&mut self) {
        if cfg!(feature = "debug") {
            use core::fmt::Write;
            let mut serial = crate::serial::SerialPort::new();
            serial.init();
            let _ = write!(serial, "DBG ELF META: release pages={}\n", self.len);
        }
        let pool_start = core::ptr::addr_of_mut!(PROVENANCE_POOL) as usize;
        let chunk_size = core::mem::size_of::<LoadedPageChunk>();
        let mut chunk = self.head;
        while !chunk.is_null() {
            let next = (*chunk).next;
            let offset = chunk as usize - pool_start;
            if offset % chunk_size == 0 {
                let index = offset / chunk_size;
                if index < PROVENANCE_CHUNK_COUNT {
                    PROVENANCE_FREE_MASK |= 1u64 << index;
                    (*chunk).next = core::ptr::null_mut();
                    (*chunk).used = 0;
                }
            }
            chunk = next;
        }
        self.head = core::ptr::null_mut();
        self.tail = core::ptr::null_mut();
        self.len = 0;
        if cfg!(feature = "debug") {
            use core::fmt::Write;
            let mut serial = crate::serial::SerialPort::new();
            serial.init();
            let _ = write!(serial, "DBG ELF META: release complete\n");
        }
    }
}

impl Drop for LoadedPages {
    fn drop(&mut self) {
        unsafe {
            self.release_metadata();
        }
    }
}

/// DEBUG-only lower-level regression for the provenance arena.  This avoids
/// depending on VIBIT shell timing while exercising the >256-page capacity and
/// reclamation contract used by the ELF loader.
pub fn test_provenance_arena(serial: &mut crate::serial::SerialPort) {
    let mut ok = true;
    {
        let mut pages = LoadedPages::new();
        for index in 0..257u64 {
            match pages.reserve() {
                Ok(slot) => unsafe {
                    (*slot).vaddr = 0x0200_0000 + index * 0x1000;
                    (*slot).paddr = 0x0040_0000 + index * 0x1000;
                },
                Err(_) => {
                    ok = false;
                    break;
                }
            }
        }
        ok &= pages.len == 257;
        ok &= pages.find(0x0200_0000) == Some(0x0040_0000);
        ok &= pages.find(0x0200_0000 + 256 * 0x1000) == Some(0x0040_0000 + 256 * 0x1000);
    }
    {
        let mut pages = LoadedPages::new();
        for _ in 0..257 {
            if pages.reserve().is_err() {
                ok = false;
                break;
            }
        }
        ok &= pages.len == 257;
    }
    if ok {
        serial.writestrs(["ELF: provenance arena >256 pages and cleanup OK.\\n"].as_slice());
    } else {
        serial.writestrs(["ELF: provenance arena regression FAILED.\\n"].as_slice());
    }
}

fn rollback_error(
    error: ElfError,
    pages: &mut LoadedPages,
    pmm: &mut PmmAllocator,
    pml4_phys: u64,
) -> Result<u64, ElfError> {
    unsafe {
        pages.rollback(pmm, pml4_phys);
    }
    Err(error)
}

/// Load an ELF64 executable into memory.
///
/// Scans program headers for PT_LOAD segments, allocates physical pages,
/// maps them at the requested virtual addresses, and copies segment data
/// from the ELF buffer.
///
/// Returns the entry point virtual address on success.
pub fn load(data: &[u8], pmm: &mut PmmAllocator, pml4_phys: u64) -> Result<u64, ElfError> {
    // === Parse ELF header ===
    if data.len() < core::mem::size_of::<Elf64Ehdr>() {
        return Err(ElfError::Truncated);
    }

    let ehdr: &Elf64Ehdr = unsafe { &*(data.as_ptr() as *const Elf64Ehdr) };

    // Validate magic
    if ehdr.e_ident[0..4] != ELF_MAGIC {
        return Err(ElfError::BadMagic);
    }
    // Validate 64-bit
    if ehdr.e_ident[4] != ELFCLASS64 {
        return Err(ElfError::BadClass);
    }
    // Validate little-endian
    if ehdr.e_ident[5] != ELFDATA2LSB {
        return Err(ElfError::BadEndian);
    }
    // Validate x86-64
    if ehdr.e_machine != EM_X86_64 {
        return Err(ElfError::BadMachine);
    }
    // Validate executable type
    if ehdr.e_type != ET_EXEC {
        return Err(ElfError::BadType);
    }

    // === Parse program headers ===
    let phoff = ehdr.e_phoff as usize;
    let phentsize = ehdr.e_phentsize as usize;
    let phnum = ehdr.e_phnum as usize;

    // Sanity check: program headers must be within the data slice
    let ph_table_size = match phnum.checked_mul(phentsize) {
        Some(size) => size,
        None => return Err(ElfError::Truncated),
    };
    if phentsize < core::mem::size_of::<Elf64Phdr>() {
        return Err(ElfError::Truncated);
    }
    if phoff
        .checked_add(ph_table_size)
        .map_or(true, |end| end > data.len())
    {
        return Err(ElfError::Truncated);
    }

    if cfg!(feature = "debug") {
        let mut serial = crate::serial::SerialPort::new();
        serial.init();
        let _ = write!(serial,
            "DBG ELF: entry=0x{:016x} phoff=0x{:016x} phentsize={} phnum={} target=0x0000000002000000\n",
            ehdr.e_entry, ehdr.e_phoff, ehdr.e_phentsize, ehdr.e_phnum);
        for i in 0..phnum {
            let phdr_offset = phoff + i * phentsize;
            let phdr: &Elf64Phdr =
                unsafe { &*(data.as_ptr().add(phdr_offset) as *const Elf64Phdr) };
            if phdr.p_type == PT_LOAD {
                let seg_end = phdr.p_vaddr.saturating_add(phdr.p_memsz);
                let covers_entry = phdr.p_vaddr <= 0x2000000 && 0x2000000 < seg_end;
                let rounded_start = phdr.p_vaddr & !0xFFF;
                let rounded_end = seg_end.saturating_add(0xFFF) & !0xFFF;
                let _ = write!(serial,
                    "DBG ELF: PT_LOAD[{}] vaddr=0x{:016x} end=0x{:016x} pages=[0x{:016x},0x{:016x}) off=0x{:016x} filesz=0x{:x} memsz=0x{:x} flags=0x{:x}{}\n",
                    i, phdr.p_vaddr, seg_end, rounded_start, rounded_end, phdr.p_offset, phdr.p_filesz, phdr.p_memsz,
                    phdr.p_flags, if covers_entry { " COVER_ENTRY" } else { "" });
            }
        }
    }

    // This tracker is local to one load call. Existing mappings from fork or
    // the old image are never treated as loader-owned provenance.
    let mut loaded_pages = LoadedPages::new();

    for i in 0..phnum {
        let phdr_offset = match phoff.checked_add(match i.checked_mul(phentsize) {
            Some(offset) => offset,
            None => return rollback_error(ElfError::Truncated, &mut loaded_pages, pmm, pml4_phys),
        }) {
            Some(offset) => offset,
            None => return rollback_error(ElfError::Truncated, &mut loaded_pages, pmm, pml4_phys),
        };
        if phdr_offset
            .checked_add(core::mem::size_of::<Elf64Phdr>())
            .map_or(true, |end| end > data.len())
        {
            return rollback_error(ElfError::Truncated, &mut loaded_pages, pmm, pml4_phys);
        }

        let phdr: &Elf64Phdr = unsafe { &*(data.as_ptr().add(phdr_offset) as *const Elf64Phdr) };

        if phdr.p_type != PT_LOAD {
            continue; // Skip non-loadable segments
        }

        let vaddr = phdr.p_vaddr;
        let filesz = phdr.p_filesz as usize;
        let memsz = phdr.p_memsz as usize;

        // --- Map pages for this segment ---
        // Segments may not be page-aligned. We need to map all pages that
        // contain the segment, then zero-fill BSS and copy data.

        let seg_start = vaddr;
        let seg_end = match vaddr.checked_add(memsz as u64) {
            Some(end) => end,
            None => return rollback_error(ElfError::Truncated, &mut loaded_pages, pmm, pml4_phys),
        };

        // Round start down to page boundary, end up to page boundary
        let page_start = seg_start & !0xFFF;
        let page_end = match seg_end.checked_add(0xFFF) {
            Some(end) => end & !0xFFF,
            None => return rollback_error(ElfError::Truncated, &mut loaded_pages, pmm, pml4_phys),
        };

        // Map every segment page RW while loading. Permission relaxation is not
        // enabled yet, and NX cannot be used until EFER.NXE is enabled.
        let page_flags = paging::PAGE_PRESENT | paging::PAGE_USER | paging::PAGE_WRITABLE;

        // Allocate and map each segment page RW while loading. A page is
        // allocated, mapped, and zeroed exactly once per load call. Later
        // PT_LOAD segments reuse only pages recorded in loaded_pages.
        //
        // CRITICAL: Disable interrupts during page table operations to prevent
        // the PIT IRQ from triggering a context switch. A CR3 change during
        // map_4k_target would corrupt the per-process page tables.
        let saved_if: u64;
        unsafe {
            core::arch::asm!("pushfq; pop {}; cli", out(reg) saved_if, options(nostack));
        }
        let _irq_guard = IrqGuard(saved_if); // restores STI on exit
        let mut vaddr_page = page_start;
        while vaddr_page < page_end {
            let tracked = loaded_pages.find(vaddr_page);

            if let Some(paddr) = tracked {
                if cfg!(feature = "debug") {
                    let mut serial = crate::serial::SerialPort::new();
                    serial.init();
                    let _ =
                        write!(serial,
                        "DBG ELF REUSE: vaddr=0x{:016x} paddr=0x{:016x} target_pml4=0x{:016x}\n",
                        vaddr_page, paddr, pml4_phys);
                    paging::debug_dump_entry_evidence("elf PT_LOAD reuse", vaddr_page, pml4_phys);
                }
            } else {
                let slot = match loaded_pages.reserve() {
                    Ok(slot) => slot,
                    Err(error) => return rollback_error(error, &mut loaded_pages, pmm, pml4_phys),
                };
                let p = pmm.alloc();
                if p.is_null() {
                    loaded_pages.cancel_reservation();
                    return rollback_error(ElfError::Oom, &mut loaded_pages, pmm, pml4_phys);
                }
                let paddr = p as u64;
                paging::map_4k_target(vaddr_page, paddr, page_flags, pmm, pml4_phys);
                paging::invlpg(vaddr_page);
                unsafe {
                    (*slot).vaddr = vaddr_page;
                    (*slot).paddr = paddr;
                }
                paging::debug_dump_map_site(
                    "elf PT_LOAD map",
                    vaddr_page,
                    paddr,
                    page_flags,
                    pml4_phys,
                );
                // Zero through the target virtual mapping, never through an
                // assumed physical alias.
                unsafe {
                    core::ptr::write_bytes(vaddr_page as *mut u8, 0, 4096);
                }
                paging::debug_dump_entry_evidence("elf page zero", vaddr_page, pml4_phys);
                if cfg!(feature = "debug") {
                    let mut serial = crate::serial::SerialPort::new();
                    serial.init();
                    let _ = write!(serial,
                        "DBG ELF TRACK: new vaddr=0x{:016x} paddr=0x{:016x} target_pml4=0x{:016x}\n",
                        vaddr_page, paddr, pml4_phys);
                }
            }

            vaddr_page = match vaddr_page.checked_add(4096) {
                Some(next) => next,
                None => {
                    return rollback_error(ElfError::Truncated, &mut loaded_pages, pmm, pml4_phys)
                }
            };
        }

        // Copy data from file (the portion that has file backing)
        let src_offset = phdr.p_offset as usize;
        if filesz > 0 {
            // Check bounds
            let src_end = match src_offset.checked_add(filesz) {
                Some(end) => end,
                None => {
                    return rollback_error(ElfError::Truncated, &mut loaded_pages, pmm, pml4_phys)
                }
            };
            if src_end > data.len() {
                return rollback_error(ElfError::Truncated, &mut loaded_pages, pmm, pml4_phys);
            }
            let src = &data[src_offset..src_end];

            let copy_len = filesz.min(memsz);
            if cfg!(feature = "debug") {
                let mut serial = crate::serial::SerialPort::new();
                serial.init();
                let _ = write!(serial, "DBG ELF COPY BEFORE: active_cr3=0x{:016x} target_pml4=0x{:016x} dst=0x{:016x} src_off=0x{:x} len=0x{:x} src=", paging::read_cr3(), pml4_phys, seg_start, src_offset, copy_len);
                for i in 0..core::cmp::min(copy_len, 16) {
                    let _ = write!(serial, "{:02x}", src[i]);
                }
                serial.writestrs(&["\n"]);
            }

            // Copy segment data into mapped pages.
            // Since segments may not be page-aligned, calculate the offset
            // within the first page.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    src.as_ptr(),
                    (seg_start) as *mut u8, // vaddr after map_4k is accessible
                    copy_len,
                );
            }
            if cfg!(feature = "debug") {
                paging::debug_dump_entry_evidence("elf copy after", seg_start, pml4_phys);
            }
        }

        if cfg!(feature = "debug") {
            paging::debug_dump_entry_evidence("after PT_LOAD", 0x2000000, pml4_phys);
        }

        // BSS (memsz > filesz) is already zeroed since we zeroed all pages.
        // If the segment is read-only, remove WRITABLE flag now.
        // NOTE: Relaxation disabled for debugging — all pages stay RW.
        // This is acceptable since EFER.NXE is not set (no execute-disable bit).
    }

    if cfg!(feature = "debug") {
        paging::debug_dump_walk("after elf::load", 0x2000000, pml4_phys);
    }
    let entry = ehdr.e_entry;
    Ok(entry)
}
