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
    e_ident: [u8; 16],   // ELF identification
    e_type: u16,          // Object file type
    e_machine: u16,       // Architecture
    e_version: u32,       // Object file version
    e_entry: u64,         // Entry point virtual address
    e_phoff: u64,         // Program header offset
    e_shoff: u64,         // Section header offset
    e_flags: u32,         // Processor-specific flags
    e_ehsize: u16,        // ELF header size
    e_phentsize: u16,     // Size of program header entry
    e_phnum: u16,         // Number of program header entries
    e_shentsize: u16,     // Size of section header entry
    e_shnum: u16,         // Number of section header entries
    e_shstrndx: u16,      // Section header string table index
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Elf64Phdr {
    p_type: u32,    // Segment type
    p_flags: u32,   // Segment flags
    p_offset: u64,  // Segment file offset
    p_vaddr: u64,   // Segment virtual address
    p_paddr: u64,   // Segment physical address
    p_filesz: u64,  // Segment size in file
    p_memsz: u64,   // Segment size in memory
    p_align: u64,   // Segment alignment
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
}

/// Provenance for a page allocated by one ELF load operation.
#[derive(Clone, Copy)]
struct LoadedPage {
    vaddr: u64,
    paddr: u64,
}

// ELF images in this kernel are small and statically loaded. Keeping this
// bounded avoids introducing allocator or hash-map behavior into exec().
const MAX_LOADED_PAGES: usize = 256;

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
    if phoff.saturating_add(phnum.saturating_mul(phentsize)) > data.len() {
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
    let mut loaded_pages: [Option<LoadedPage>; MAX_LOADED_PAGES] = [None; MAX_LOADED_PAGES];
    let mut loaded_page_count = 0usize;

    for i in 0..phnum {
        let phdr_offset = phoff + i * phentsize;
        if phdr_offset + core::mem::size_of::<Elf64Phdr>() > data.len() {
            return Err(ElfError::Truncated);
        }

        let phdr: &Elf64Phdr = unsafe { &*(data.as_ptr().add(phdr_offset) as *const Elf64Phdr) };

        if phdr.p_type != PT_LOAD {
            continue;  // Skip non-loadable segments
        }

        let vaddr = phdr.p_vaddr;
        let filesz = phdr.p_filesz as usize;
        let memsz = phdr.p_memsz as usize;

        // --- Map pages for this segment ---
        // Segments may not be page-aligned. We need to map all pages that
        // contain the segment, then zero-fill BSS and copy data.

        let seg_start = vaddr;
        let seg_end = vaddr + memsz as u64;

        // Round start down to page boundary, end up to page boundary
        let page_start = seg_start & !0xFFF;
        let page_end = (seg_end + 0xFFF) & !0xFFF;

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
            let mut tracked = None;
            for slot in loaded_pages.iter() {
                if let Some(page) = slot {
                    if page.vaddr == vaddr_page {
                        tracked = Some(page.paddr);
                        break;
                    }
                }
            }

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
                if loaded_page_count == MAX_LOADED_PAGES {
                    return Err(ElfError::Oom);
                }
                let p = pmm.alloc();
                if p.is_null() {
                    return Err(ElfError::Oom);
                }
                let paddr = p as u64;
                paging::map_4k_target(vaddr_page, paddr, page_flags, pmm, pml4_phys);
                paging::invlpg(vaddr_page);
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
                loaded_pages[loaded_page_count] = Some(LoadedPage {
                    vaddr: vaddr_page,
                    paddr,
                });
                loaded_page_count += 1;
                if cfg!(feature = "debug") {
                    let mut serial = crate::serial::SerialPort::new();
                    serial.init();
                    let _ = write!(serial,
                        "DBG ELF TRACK: new vaddr=0x{:016x} paddr=0x{:016x} target_pml4=0x{:016x}\n",
                        vaddr_page, paddr, pml4_phys);
                }
            }

            vaddr_page += 4096;
        }

        // Copy data from file (the portion that has file backing)
        let src_offset = phdr.p_offset as usize;
        if filesz > 0 {
            // Check bounds
            if src_offset + filesz > data.len() {
                return Err(ElfError::Truncated);
            }
            let src = &data[src_offset..src_offset + filesz];

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
                    (seg_start) as *mut u8,  // vaddr after map_4k is accessible
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
    Ok(ehdr.e_entry)
}
