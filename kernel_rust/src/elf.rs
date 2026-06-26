//==============================================================================
// elf.rs — ELF64 loader
//
// Parses ELF64 executables, loads PT_LOAD segments into memory, and returns
// the entry point.
//==============================================================================

// ELF loader — wired into sys_exec() for ELF64 binary support

use crate::paging;
use crate::pmm::PmmAllocator;

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

/// Load an ELF64 executable into memory.
///
/// Scans program headers for PT_LOAD segments, allocates physical pages,
/// maps them at the requested virtual addresses, and copies segment data
/// from the ELF buffer.
///
/// Returns the entry point virtual address on success.
pub fn load(data: &[u8], pmm: &mut PmmAllocator) -> Result<u64, ElfError> {
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

        // Determine page flags from p_flags:
        //   PF_R (4)  → PAGE_PRESENT
        //   PF_W (2)  → PAGE_WRITABLE
        //   PF_X (1)  → if NOT executable, add PAGE_NO_EXEC
        let pf = phdr.p_flags;
        let mut page_flags = paging::PAGE_PRESENT | paging::PAGE_USER;
        if pf & 2 != 0 { page_flags |= paging::PAGE_WRITABLE; }
        // If NOT executable, set NX bit
        if pf & 1 == 0 { page_flags |= paging::PAGE_NO_EXEC; }

        // Allocate and map each page in the segment's virtual address range
        let mut vaddr_page = page_start;
        while vaddr_page < page_end {
            let phys = pmm.alloc();
            if phys.is_null() {
                return Err(ElfError::Oom);
            }
            paging::map_4k(vaddr_page, phys as u64, page_flags, pmm);
            paging::invlpg(vaddr_page);

            // Zero the entire page first
            unsafe {
                core::ptr::write_bytes(phys, 0, 4096);
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

            // Copy segment data into mapped pages.
            // Since segments may not be page-aligned, calculate the offset
            // within the first page.
            let copy_len = filesz.min(memsz);
            unsafe {
                core::ptr::copy_nonoverlapping(
                    src.as_ptr(),
                    (seg_start) as *mut u8,  // vaddr after map_4k is accessible
                    copy_len,
                );
            }
        }
        // BSS (memsz > filesz) is already zeroed since we zeroed all pages
    }

    Ok(ehdr.e_entry)
}
