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
const PF_X: u32 = 1;

/// x86-64 architecture.
const EM_X86_64: u16 = 0x3E;

pub const ELF_STACK_PAGES: u64 = 16;
pub const ELF_STACK_TOP_PAGE: u64 = 0x2200000;
pub const ELF_STACK_LOW: u64 = ELF_STACK_TOP_PAGE - ELF_STACK_PAGES * 0x1000;
pub const USER_ADDR_LIMIT: u64 = 0x0000_8000_0000_0000;

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
    InvalidAddress,
    FixedStackOverlap,
    MappingCollision,
    InvalidEntry,
    Truncated,
    Oom,
    MetadataOom,
}

/// Per-load fault controls used only by the DEBUG rollback regression.
///
/// The normal loader always uses the disabled configuration.  Keeping the
/// counters on the load stack avoids a mutable global fault switch leaking
/// into another load or into release builds.
struct LoadFailureInjection {
    #[cfg(feature = "debug")]
    suppress_debug_trace: bool,
    #[cfg(feature = "debug")]
    pmm_fail_after: Option<usize>,
    #[cfg(feature = "debug")]
    metadata_fail_after: Option<usize>,
    #[cfg(feature = "debug")]
    pmm_attempts: usize,
    #[cfg(feature = "debug")]
    metadata_attempts: usize,
    #[cfg(feature = "debug")]
    mapped_paddrs: [u64; 4],
    #[cfg(feature = "debug")]
    mapped_vaddrs: [u64; 4],
    #[cfg(feature = "debug")]
    mapped_count: usize,
}

impl LoadFailureInjection {
    const fn disabled() -> Self {
        Self {
            #[cfg(feature = "debug")]
            suppress_debug_trace: false,
            #[cfg(feature = "debug")]
            pmm_fail_after: None,
            #[cfg(feature = "debug")]
            metadata_fail_after: None,
            #[cfg(feature = "debug")]
            pmm_attempts: 0,
            #[cfg(feature = "debug")]
            metadata_attempts: 0,
            #[cfg(feature = "debug")]
            mapped_paddrs: [0; 4],
            #[cfg(feature = "debug")]
            mapped_vaddrs: [0; 4],
            #[cfg(feature = "debug")]
            mapped_count: 0,
        }
    }

    #[inline(always)]
    fn should_fail_pmm(&mut self) -> bool {
        #[cfg(feature = "debug")]
        {
            let fail = self.pmm_fail_after == Some(self.pmm_attempts);
            self.pmm_attempts += 1;
            return fail;
        }
        #[cfg(not(feature = "debug"))]
        {
            false
        }
    }

    #[inline(always)]
    fn should_fail_metadata(&mut self) -> bool {
        #[cfg(feature = "debug")]
        {
            let fail = self.metadata_fail_after == Some(self.metadata_attempts);
            self.metadata_attempts += 1;
            return fail;
        }
        #[cfg(not(feature = "debug"))]
        {
            false
        }
    }

    #[inline(always)]
    fn trace_enabled(&self) -> bool {
        #[cfg(feature = "debug")]
        {
            !self.suppress_debug_trace
        }
        #[cfg(not(feature = "debug"))]
        {
            false
        }
    }

    #[cfg(feature = "debug")]
    fn record_mapping(&mut self, vaddr: u64, paddr: u64) {
        if self.mapped_count < self.mapped_paddrs.len() {
            self.mapped_vaddrs[self.mapped_count] = vaddr;
            self.mapped_paddrs[self.mapped_count] = paddr;
        }
        self.mapped_count += 1;
    }
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
                if let Some(entry) = paging::unmap_and_free_empty(page.vaddr, pmm) {
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

#[cfg(feature = "debug")]
// paging::test leaves this user page-table branch allocated after removing its
// leaf, so the rollback cases do not allocate page-table frames of their own.
// Keep the fixture outside the kernel's low identity map; otherwise the
// inherited-page precondition is already satisfied before the test maps it.
const ROLLBACK_TEST_BASE: u64 = 0x0300_0000;
#[cfg(feature = "debug")]
const ROLLBACK_TEST_INHERITED: u64 = ROLLBACK_TEST_BASE + 0x3000;

#[cfg(feature = "debug")]
fn put_u16(image: &mut [u8], offset: usize, value: u16) {
    image[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

#[cfg(feature = "debug")]
fn put_u32(image: &mut [u8], offset: usize, value: u32) {
    image[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

#[cfg(feature = "debug")]
fn put_u64(image: &mut [u8], offset: usize, value: u64) {
    image[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

#[cfg(feature = "debug")]
fn build_rollback_elf(truncated_later_segment: bool) -> Option<*mut u8> {
    let image_ptr = crate::kmm::kmalloc(0x1008);
    if image_ptr.is_null() {
        return None;
    }
    let mut image = unsafe { core::slice::from_raw_parts_mut(image_ptr, 0x1008) };
    for byte in image.iter_mut() {
        *byte = 0;
    }
    image[0..16].copy_from_slice(&[
        0x7f, b'E', b'L', b'F', ELFCLASS64, ELFDATA2LSB, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ]);
    put_u16(&mut image, 16, ET_EXEC);
    put_u16(&mut image, 18, EM_X86_64);
    put_u32(&mut image, 20, 1);
    put_u64(&mut image, 24, ROLLBACK_TEST_BASE);
    put_u64(&mut image, 32, 64);
    put_u16(&mut image, 52, 64);
    put_u16(&mut image, 54, 56);
    put_u16(&mut image, 56, 2);

    // PT_LOAD[0] is valid and guarantees that rollback starts after a map.
    put_u32(&mut image, 64, PT_LOAD);
    put_u32(&mut image, 68, 5);
    put_u64(&mut image, 72, 0x1000);
    put_u64(&mut image, 80, ROLLBACK_TEST_BASE);
    put_u64(&mut image, 96, 1);
    put_u64(&mut image, 104, 0x1000);

    // PT_LOAD[1] maps a second page.  Its file byte is either valid or just
    // beyond the image so the loader reports Truncated after both maps exist.
    let second = 64 + 56;
    put_u32(&mut image, second, PT_LOAD);
    put_u32(&mut image, second + 4, 5);
    put_u64(
        &mut image,
        second + 8,
        if truncated_later_segment { 0x1008 } else { 0x1001 },
    );
    put_u64(&mut image, second + 16, ROLLBACK_TEST_BASE + 0x1000);
    put_u64(&mut image, second + 32, 1);
    put_u64(&mut image, second + 40, 0x1000);
    image[0x1000] = 0x90;
    image[0x1001] = 0x90;
    Some(image_ptr)
}

#[cfg(feature = "debug")]
fn read_rflags() -> u64 {
    let flags: u64;
    unsafe {
        core::arch::asm!("pushfq; pop {}", out(reg) flags, options(nomem, preserves_flags));
    }
    flags
}

#[cfg(feature = "debug")]
fn error_kind(error: &ElfError) -> u8 {
    match error {
        ElfError::Truncated => 1,
        ElfError::Oom => 2,
        ElfError::MetadataOom => 3,
        ElfError::InvalidAddress
        | ElfError::FixedStackOverlap
        | ElfError::MappingCollision
        | ElfError::InvalidEntry
        | ElfError::BadMagic
        | ElfError::BadClass
        | ElfError::BadEndian
        | ElfError::BadMachine
        | ElfError::BadType => 0,
    }
}

#[cfg(feature = "debug")]
fn metadata_is_reusable() -> bool {
    let mut pages = LoadedPages::new();
    let mut ok = true;
    for _ in 0..(PROVENANCE_CHUNK_CAP * PROVENANCE_CHUNK_COUNT) {
        if pages.reserve().is_err() {
            ok = false;
            break;
        }
    }
    drop(pages);
    ok
}

#[cfg(feature = "debug")]
fn frames_are_reusable(pmm: &mut PmmAllocator, injection: &LoadFailureInjection) -> bool {
    let count = core::cmp::min(injection.mapped_count, injection.mapped_paddrs.len());
    let mut actual = [0u64; 4];
    let mut ok = true;
    for slot in actual.iter_mut().take(count) {
        let page = pmm.alloc();
        if page.is_null() {
            ok = false;
            break;
        }
        *slot = page as u64;
    }

    let mut matched = [false; 4];
    if ok {
        for page in actual.iter().take(count) {
            let mut found = false;
            for (index, expected) in injection.mapped_paddrs.iter().take(count).enumerate() {
                if !matched[index] && *expected == *page {
                    matched[index] = true;
                    found = true;
                    break;
                }
            }
            ok &= found;
        }
    }
    for page in actual.iter().take(count) {
        if *page != 0 {
            pmm.free(*page as *mut u8);
        }
    }
    ok && matched.iter().take(count).all(|matched| *matched)
}

#[cfg(feature = "debug")]
fn run_rollback_case(
    serial: &mut crate::serial::SerialPort,
    label: &str,
    image: &[u8],
    expected_error: u8,
    injection: &mut LoadFailureInjection,
    pmm: &mut PmmAllocator,
    pml4_phys: u64,
    inherited_paddr: u64,
) -> bool {
    serial.writestrs(["ELF TEST: ", label, " begin\n"].as_slice());
    let saved_cr3 = paging::read_cr3();
    let saved_flags = read_rflags();
    let result = load_with_injection(image, pmm, pml4_phys, injection);
    let error_ok = match result {
        Err(error) => error_kind(&error) == expected_error,
        Ok(_) => false,
    };
    let cr3_ok = paging::read_cr3() == saved_cr3;
    let if_ok = (read_rflags() & 0x200) == (saved_flags & 0x200);

    let count = core::cmp::min(injection.mapped_count, injection.mapped_vaddrs.len());
    let mappings_removed = injection
        .mapped_vaddrs
        .iter()
        .take(count)
        .all(|vaddr| paging::translate_in_pml4(*vaddr, pml4_phys).is_none());
    let inherited_intact =
        paging::translate_in_pml4(ROLLBACK_TEST_INHERITED, pml4_phys) == Some(inherited_paddr);
    let frames_reused = frames_are_reusable(pmm, injection);
    let metadata_reused = metadata_is_reusable();
    let ok = error_ok
        && mappings_removed
        && inherited_intact
        && frames_reused
        && metadata_reused
        && cr3_ok
        && if_ok;

    serial.writestrs(&["ELF TEST: ", label, if ok { " rollback PASS\n" } else { " rollback FAIL\n" }]);
    if cfg!(feature = "debug") && !ok {
        let mut detail = crate::serial::SerialPort::new();
        detail.init();
        let _ = write!(
            detail,
            "DBG ELF TEST: {} error={} maps={} inherited={} frames={} metadata={} cr3={} if={}\n",
            label,
            error_ok,
            mappings_removed,
            inherited_intact,
            frames_reused,
            metadata_reused,
            cr3_ok,
            if_ok
        );
    }
    ok
}

/// DEBUG-only transactional rollback regression.  The cases use per-load
/// injection state and a pre-existing mapping in the same page-table branch,
/// so no allocator or fault-global state is left behind between cases.
#[cfg(feature = "debug")]
pub fn test_provenance_failures(
    pmm: &mut PmmAllocator,
    serial: &mut crate::serial::SerialPort,
) -> bool {
    serial.writestrs(["ELF TEST: rollback setup begin\n"].as_slice());
    let pml4_phys = paging::read_cr3();
    let inherited_before = paging::translate_in_pml4(ROLLBACK_TEST_INHERITED, pml4_phys);
    let inherited_page = pmm.alloc();
    if inherited_page.is_null() || inherited_before.is_some() {
        serial.writestrs(["ELF TEST: setup rollback FAIL\n"].as_slice());
        if !inherited_page.is_null() {
            pmm.free(inherited_page);
        }
        return false;
    }
    paging::map_4k(
        ROLLBACK_TEST_INHERITED,
        inherited_page as u64,
        paging::PAGE_USER_RW,
        pmm,
    );
    paging::invlpg(ROLLBACK_TEST_INHERITED);
    serial.writestrs(["ELF TEST: rollback setup ready\n"].as_slice());

    let image_ptr = match build_rollback_elf(true) {
        Some(ptr) => ptr,
        None => {
            serial.writestrs(["ELF TEST: rollback fixture allocation FAIL\n"].as_slice());
            if let Some(entry) = paging::unmap_and_free_empty(ROLLBACK_TEST_INHERITED, pmm) {
                pmm.free((entry & 0x000F_FFFF_FFFF_F000u64) as *mut u8);
            }
            return false;
        }
    };
    serial.writestrs(["ELF TEST: rollback fixture truncated ready\n"].as_slice());

    let mut ok = true;
    {
        let mut injection = LoadFailureInjection::disabled();
        injection.suppress_debug_trace = true;
        let image = unsafe { core::slice::from_raw_parts(image_ptr, 0x1008) };
        ok &= run_rollback_case(
            serial,
            "truncated-later-segment",
            image,
            1,
            &mut injection,
            pmm,
            pml4_phys,
            inherited_page as u64,
        );
    }
    crate::kmm::kfree(image_ptr);
    let image_ptr = match build_rollback_elf(false) {
        Some(ptr) => ptr,
        None => {
            if let Some(entry) = paging::unmap_and_free_empty(ROLLBACK_TEST_INHERITED, pmm) {
                pmm.free((entry & 0x000F_FFFF_FFFF_F000u64) as *mut u8);
            }
            serial.writestrs(["ELF TEST: rollback fixture allocation FAIL\n"].as_slice());
            return false;
        }
    };
    serial.writestrs(["ELF TEST: rollback fixture valid ready\n"].as_slice());
    {
        let mut injection = LoadFailureInjection::disabled();
        injection.suppress_debug_trace = true;
        injection.pmm_fail_after = Some(1);
        let image = unsafe { core::slice::from_raw_parts(image_ptr, 0x1008) };
        ok &= run_rollback_case(
            serial,
            "pmm-exhaustion",
            image,
            2,
            &mut injection,
            pmm,
            pml4_phys,
            inherited_page as u64,
        );
    }
    {
        let mut injection = LoadFailureInjection::disabled();
        injection.suppress_debug_trace = true;
        injection.metadata_fail_after = Some(1);
        let image = unsafe { core::slice::from_raw_parts(image_ptr, 0x1008) };
        ok &= run_rollback_case(
            serial,
            "metadata-exhaustion",
            image,
            3,
            &mut injection,
            pmm,
            pml4_phys,
            inherited_page as u64,
        );
    }
    crate::kmm::kfree(image_ptr);

    if let Some(entry) = paging::unmap_and_free_empty(ROLLBACK_TEST_INHERITED, pmm) {
        pmm.free((entry & 0x000F_FFFF_FFFF_F000u64) as *mut u8);
    } else {
        ok = false;
    }
    if ok {
        serial.writestrs(["ELF TEST: rollback suite PASS\n"].as_slice());
    } else {
        serial.writestrs(["ELF TEST: rollback suite FAIL\n"].as_slice());
    }

    // Exercise the shared create-root -> ELF -> real-stack builder.  The hook
    // fails after the first stack page has been mapped, so cleanup must reclaim
    // the image leaf, stack leaf, and every constructor-owned table while the
    // old active root and inherited mapping remain untouched.
    let mut expected_frames = [0u64; 64];
    let mut frame_window_ok = true;
    for frame in &mut expected_frames {
        *frame = pmm.alloc() as u64;
        if *frame == 0 {
            frame_window_ok = false;
            break;
        }
    }
    for frame in &expected_frames {
        if *frame != 0 {
            pmm.free(*frame as *mut u8);
        }
    }
    let hook_image = build_rollback_elf(false);
    let old_cr3 = paging::read_cr3();
    let old_mapping = paging::translate_in_pml4(ROLLBACK_TEST_INHERITED, old_cr3);
    let hook_result = match hook_image {
        Some(image_ptr) => {
            let image = unsafe { core::slice::from_raw_parts(image_ptr, 0x1008) };
            let result = build_exec_image_with_stack_failure(image, pmm, 1);
            crate::kmm::kfree(image_ptr);
            result
        }
        None => Err((ElfError::Oom, 0)),
    };
    let mut recycled_frames = [0u64; 64];
    for frame in &mut recycled_frames {
        *frame = pmm.alloc() as u64;
        if *frame == 0 {
            frame_window_ok = false;
            break;
        }
    }
    for frame in &recycled_frames {
        if *frame != 0 {
            pmm.free(*frame as *mut u8);
        }
    }
    let mut expected_sorted = expected_frames;
    let mut recycled_sorted = recycled_frames;
    expected_sorted.sort_unstable();
    recycled_sorted.sort_unstable();
    let hook_ok = matches!(hook_result, Err((ElfError::Oom, 1)))
        && paging::read_cr3() == old_cr3
        && paging::translate_in_pml4(ROLLBACK_TEST_INHERITED, old_cr3) == old_mapping
        && frame_window_ok
        && expected_sorted == recycled_sorted;
    serial.writestrs(
        [
            "ELF TEST: stack-allocation rollback ",
            if hook_ok { "PASS\n" } else { "FAIL\n" },
        ]
        .as_slice(),
    );
    ok && hook_ok
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
    let mut injection = LoadFailureInjection::disabled();
    load_with_injection(data, pmm, pml4_phys, &mut injection)
}

/// A fully built but not-yet-published ELF image.  Ownership of `root` stays
/// with this value until the caller atomically installs all process state.
pub struct ExecImage {
    pub root: paging::Pml4Handle,
    pub entry: u64,
    pub user_rsp: u64,
    pub stack_low: u64,
}

fn build_exec_image_inner(
    data: &[u8],
    pmm: &mut PmmAllocator,
    _stack_fail_after: Option<u64>,
    stack_pages_mapped: &mut usize,
    _suppress_debug_trace: bool,
) -> Result<ExecImage, ElfError> {
    let root = paging::create_pml4(pmm, false).ok_or(ElfError::Oom)?;
    let mut injection = LoadFailureInjection::disabled();
    #[cfg(feature = "debug")]
    {
        injection.suppress_debug_trace = _suppress_debug_trace;
    }
    let entry = match load_with_injection(data, pmm, root.phys, &mut injection) {
        Ok(entry) => entry,
        Err(error) => {
            paging::destroy_uncommitted_pml4(root, pmm);
            return Err(error);
        }
    };

    let mut page_addr = ELF_STACK_TOP_PAGE;
    for _stack_index in 0..ELF_STACK_PAGES {
        #[cfg(feature = "debug")]
        if _stack_fail_after == Some(_stack_index) {
            paging::destroy_uncommitted_pml4(root, pmm);
            return Err(ElfError::Oom);
        }
        if paging::translate_in_pml4(page_addr, root.phys).is_some() {
            paging::destroy_uncommitted_pml4(root, pmm);
            return Err(ElfError::MappingCollision);
        }
        let stack_page = pmm.alloc();
        if stack_page.is_null() {
            paging::destroy_uncommitted_pml4(root, pmm);
            return Err(ElfError::Oom);
        }
        if !paging::try_map_4k_target(
            page_addr,
            stack_page as u64,
            paging::PAGE_USER_RW,
            pmm,
            root.phys,
        ) {
            pmm.free(stack_page);
            paging::destroy_uncommitted_pml4(root, pmm);
            return Err(ElfError::Oom);
        }
        paging::invlpg(page_addr);
        *stack_pages_mapped += 1;
        page_addr -= 0x1000;
    }

    Ok(ExecImage {
        root,
        entry,
        user_rsp: ELF_STACK_TOP_PAGE + 0x1000,
        stack_low: ELF_STACK_LOW,
    })
}

/// Build an ELF image and its real user stack under one uncommitted root.
pub fn build_exec_image(data: &[u8], pmm: &mut PmmAllocator) -> Result<ExecImage, ElfError> {
    let mut stack_pages_mapped = 0;
    build_exec_image_inner(data, pmm, None, &mut stack_pages_mapped, false)
}

#[cfg(feature = "debug")]
fn build_exec_image_with_stack_failure(
    data: &[u8],
    pmm: &mut PmmAllocator,
    fail_after: u64,
) -> Result<ExecImage, (ElfError, usize)> {
    let mut stack_pages_mapped = 0;
    match build_exec_image_inner(data, pmm, Some(fail_after), &mut stack_pages_mapped, true) {
        Ok(image) => Ok(image),
        Err(error) => Err((error, stack_pages_mapped)),
    }
}

fn load_with_injection(
    data: &[u8],
    pmm: &mut PmmAllocator,
    pml4_phys: u64,
    injection: &mut LoadFailureInjection,
) -> Result<u64, ElfError> {
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

    // Validate the complete image before allocating or mapping anything.  The
    // attempted root is transaction-owned, but rejecting bad ranges up front
    // prevents a malformed image from ever colliding with the fixed stack or
    // crossing the supported lower-half user range.
    let mut entry_executable = false;
    for i in 0..phnum {
        let phdr_offset = phoff
            .checked_add(i.checked_mul(phentsize).ok_or(ElfError::Truncated)?)
            .ok_or(ElfError::Truncated)?;
        let phdr: &Elf64Phdr = unsafe { &*(data.as_ptr().add(phdr_offset) as *const Elf64Phdr) };
        if phdr.p_type != PT_LOAD {
            continue;
        }
        if phdr.p_filesz > phdr.p_memsz {
            return Err(ElfError::Truncated);
        }
        let end = phdr
            .p_vaddr
            .checked_add(phdr.p_memsz)
            .ok_or(ElfError::InvalidAddress)?;
        if phdr.p_memsz == 0 {
            continue;
        }
        if phdr.p_vaddr < crate::process::USER_CODE_ADDR || end > USER_ADDR_LIMIT {
            return Err(ElfError::InvalidAddress);
        }
        let page_start = phdr.p_vaddr & !0xFFF;
        let page_end = end.checked_add(0xFFF).ok_or(ElfError::InvalidAddress)? & !0xFFF;
        if page_start < crate::process::USER_CODE_ADDR || page_end > USER_ADDR_LIMIT {
            return Err(ElfError::InvalidAddress);
        }
        if paging::overlaps_kernel_scratch(page_start, page_end) {
            return Err(ElfError::InvalidAddress);
        }
        if page_start < ELF_STACK_TOP_PAGE + 0x1000 && page_end > ELF_STACK_LOW {
            return Err(ElfError::FixedStackOverlap);
        }
        let file_end = (phdr.p_offset as usize)
            .checked_add(phdr.p_filesz as usize)
            .ok_or(ElfError::Truncated)?;
        if file_end > data.len() {
            return Err(ElfError::Truncated);
        }
        if phdr.p_flags & PF_X != 0
            && phdr.p_vaddr <= ehdr.e_entry
            && ehdr.e_entry < end
        {
            entry_executable = true;
        }
    }
    if !entry_executable {
        return Err(ElfError::InvalidEntry);
    }

    if injection.trace_enabled() {
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
                if injection.trace_enabled() {
                    let mut serial = crate::serial::SerialPort::new();
                    serial.init();
                    let _ =
                        write!(serial,
                        "DBG ELF REUSE: vaddr=0x{:016x} paddr=0x{:016x} target_pml4=0x{:016x}\n",
                        vaddr_page, paddr, pml4_phys);
                    paging::debug_dump_entry_evidence("elf PT_LOAD reuse", vaddr_page, pml4_phys);
                }
            } else {
                if paging::translate_in_pml4(vaddr_page, pml4_phys).is_some() {
                    return rollback_error(
                        ElfError::MappingCollision,
                        &mut loaded_pages,
                        pmm,
                        pml4_phys,
                    );
                }
                let slot = match if injection.should_fail_metadata() {
                    Err(ElfError::MetadataOom)
                } else {
                    loaded_pages.reserve()
                } {
                    Ok(slot) => slot,
                    Err(error) => return rollback_error(error, &mut loaded_pages, pmm, pml4_phys),
                };
                let p = if injection.should_fail_pmm() {
                    core::ptr::null_mut()
                } else {
                    pmm.alloc()
                };
                if p.is_null() {
                    loaded_pages.cancel_reservation();
                    return rollback_error(ElfError::Oom, &mut loaded_pages, pmm, pml4_phys);
                }
                let paddr = p as u64;
                if !paging::try_map_4k_target(vaddr_page, paddr, page_flags, pmm, pml4_phys) {
                    pmm.free(p);
                    loaded_pages.cancel_reservation();
                    return rollback_error(ElfError::Oom, &mut loaded_pages, pmm, pml4_phys);
                }
                paging::invlpg(vaddr_page);
                unsafe {
                    (*slot).vaddr = vaddr_page;
                    (*slot).paddr = paddr;
                }
                #[cfg(feature = "debug")]
                injection.record_mapping(vaddr_page, paddr);
                if injection.trace_enabled() {
                    paging::debug_dump_map_site(
                        "elf PT_LOAD map",
                        vaddr_page,
                        paddr,
                        page_flags,
                        pml4_phys,
                    );
                }
                // Zero through the target virtual mapping, never through an
                // assumed physical alias.
                let saved_target_cr3 = paging::read_cr3();
                if saved_target_cr3 != pml4_phys {
                    unsafe {
                        paging::write_cr3(pml4_phys);
                    }
                }
                unsafe {
                    core::ptr::write_bytes(vaddr_page as *mut u8, 0, 4096);
                }
                if saved_target_cr3 != pml4_phys {
                    unsafe {
                        paging::write_cr3(saved_target_cr3);
                    }
                }
                if injection.trace_enabled() {
                    paging::debug_dump_entry_evidence("elf page zero", vaddr_page, pml4_phys);
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
            if injection.trace_enabled() {
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
            let saved_target_cr3 = paging::read_cr3();
            if saved_target_cr3 != pml4_phys {
                unsafe {
                    paging::write_cr3(pml4_phys);
                }
            }
            unsafe {
                core::ptr::copy_nonoverlapping(
                    src.as_ptr(),
                    (seg_start) as *mut u8, // vaddr after map_4k is accessible
                    copy_len,
                );
            }
            if saved_target_cr3 != pml4_phys {
                unsafe {
                    paging::write_cr3(saved_target_cr3);
                }
            }
            if injection.trace_enabled() {
                paging::debug_dump_entry_evidence("elf copy after", seg_start, pml4_phys);
            }
        }

        if injection.trace_enabled() {
            paging::debug_dump_entry_evidence("after PT_LOAD", 0x2000000, pml4_phys);
        }

        // BSS (memsz > filesz) is already zeroed since we zeroed all pages.
        // If the segment is read-only, remove WRITABLE flag now.
        // NOTE: Relaxation disabled for debugging — all pages stay RW.
        // This is acceptable since EFER.NXE is not set (no execute-disable bit).
    }

    if injection.trace_enabled() {
        paging::debug_dump_walk("after elf::load", 0x2000000, pml4_phys);
    }
    let entry = ehdr.e_entry;
    Ok(entry)
}
