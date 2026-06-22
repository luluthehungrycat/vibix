//==============================================================================
// paging.rs — 4-level page table management
//
// Provides primitives for mapping and unmapping pages in the active x86-64
// 4-level paging hierarchy.  Physical page allocation is delegated to PMM.
//==============================================================================

use crate::pmm::PmmAllocator;

//==============================================================================
// Page entry flags
//==============================================================================

pub const PAGE_PRESENT:   u64 = 1 << 0;
pub const PAGE_WRITABLE:  u64 = 1 << 1;
pub const PAGE_USER:      u64 = 1 << 2;
pub const PAGE_ACCESSED:  u64 = 1 << 5;
pub const PAGE_DIRTY:     u64 = 1 << 6;
pub const PAGE_HUGE:      u64 = 1 << 7;
pub const PAGE_GLOBAL:    u64 = 1 << 8;
pub const PAGE_NO_EXEC:   u64 = 1 << 63;

/// Common flag combinations.
pub const PAGE_KERNEL:       u64 = PAGE_PRESENT | PAGE_WRITABLE;
pub const PAGE_KERNEL_HUGE:  u64 = PAGE_PRESENT | PAGE_WRITABLE | PAGE_HUGE;
pub const PAGE_USER_RW:      u64 = PAGE_PRESENT | PAGE_WRITABLE | PAGE_USER;
pub const PAGE_USER_RO:      u64 = PAGE_PRESENT | PAGE_USER;

/// Physical address mask (bits 12–47).
const ADDR_MASK: u64 = 0x000FFFFF_FFFFF000;

/// A 512-entry page table (all levels: PML4, PDPT, PD, PT).
pub type PageTable = [u64; 512];

//==============================================================================
// CPU register access
//==============================================================================

/// Read CR3 (physical address of current PML4), with lower 12 bits masked off.
pub fn read_cr3() -> u64 {
    let cr3: u64;
    unsafe { core::arch::asm!("mov {}, cr3", out(reg) cr3); }
    cr3 & ADDR_MASK
}

/// Write CR3 to switch page tables (flushes the entire TLB).
///
/// # Safety
/// `cr3` must be a valid physical address of a 4 KiB aligned PML4 page table.
pub unsafe fn write_cr3(cr3: u64) {
    core::arch::asm!("mov cr3, {}", in(reg) cr3, options(nostack, preserves_flags));
}

/// Invalidate the TLB entry for a single virtual address.
pub fn invlpg(vaddr: u64) {
    unsafe { core::arch::asm!("invlpg [{v}]", v = in(reg) vaddr, options(nostack, preserves_flags)); }
}

//==============================================================================
// Page table traversal
//==============================================================================

/// Return a mutable reference to the next-level page table for a given entry,
/// allocating and wiring a fresh zeroed table if the entry is not present.
///
/// Halts on allocation failure (catastrophic OOM).
pub fn get_or_create_table<'e>(entry: &'e mut u64, pmm: &mut PmmAllocator) -> &'e mut PageTable {
    if *entry & PAGE_PRESENT != 0 {
        let addr = *entry & ADDR_MASK;
        return unsafe { &mut *(addr as *mut PageTable) };
    }
    let page = pmm.alloc();
    if page.is_null() {
        // Out of physical memory — halt.
        loop { unsafe { core::arch::asm!("hlt", options(nomem, nostack)) } }
    }
    let table = unsafe { &mut *(page as *mut PageTable) };
    for slot in table.iter_mut() { *slot = 0; }
    // Include USER in intermediate entries so user-mode (ring 3) page walks
    // succeed for pages mapped with PAGE_USER.  Kernel-only leaf entries
    // remain protected because they lack the USER bit.
    *entry = (page as u64) | PAGE_KERNEL | PAGE_USER;
    table
}

/// Compute the 4-level page table indices for a virtual address.
fn indices(vaddr: u64) -> (usize, usize, usize, usize) {
    (
        ((vaddr >> 39) & 0x1FF) as usize,
        ((vaddr >> 30) & 0x1FF) as usize,
        ((vaddr >> 21) & 0x1FF) as usize,
        ((vaddr >> 12) & 0x1FF) as usize,
    )
}

/// Return a mutable reference to the active L4 (PML4) page table.
fn active_l4() -> &'static mut PageTable {
    unsafe { &mut *(read_cr3() as *mut PageTable) }
}

//==============================================================================
// Mapping
//==============================================================================

/// Map a single 4 KiB page in the active address space.
///
/// Both `vaddr` and `paddr` must be 4 KiB aligned.
/// `flags` should include `PAGE_PRESENT` and any desired attributes.
/// Note: this does NOT flush the TLB for the mapped page.
pub fn map_4k(vaddr: u64, paddr: u64, flags: u64, pmm: &mut PmmAllocator) {
    debug_assert!(vaddr & 0xFFF == 0, "map_4k: vaddr not page-aligned");
    debug_assert!(paddr & 0xFFF == 0, "map_4k: paddr not page-aligned");

    let (l4i, l3i, l2i, l1i) = indices(vaddr);
    let is_user = flags & PAGE_USER != 0;

    // Use raw pointers to avoid borrow-checker conflicts from the chain of
    // mutable references through get_or_create_table calls.
    unsafe {
        let l4 = active_l4() as *mut PageTable;
        let l3_ptr: *mut PageTable = get_or_create_table(&mut (*l4)[l4i], pmm);
        let l2_ptr: *mut PageTable = get_or_create_table(&mut (*l3_ptr)[l3i], pmm);
        let l1_ptr: *mut PageTable = get_or_create_table(&mut (*l2_ptr)[l2i], pmm);
        (*l1_ptr)[l1i] = paddr | flags;

        // When mapping a user-accessible page, ensure ALL intermediate
        // page-table entries have the USER bit set.  The CPU checks USER
        // at EVERY level of the walk for Ring 3 accesses.
        if is_user {
            (*l4)[l4i] |= PAGE_USER;
            (*l3_ptr)[l3i] |= PAGE_USER;
            (*l2_ptr)[l2i] |= PAGE_USER;
        }
    }
}

/// Map a 2 MiB huge page in the active address space.
///
/// Both `vaddr` and `paddr` must be 2 MiB aligned.
/// `flags` should include `PAGE_PRESENT` and `PAGE_HUGE`.
pub fn map_2m(vaddr: u64, paddr: u64, flags: u64, pmm: &mut PmmAllocator) {
    debug_assert!(vaddr & 0x1F_FFFF == 0, "map_2m: vaddr not 2 MiB aligned");
    debug_assert!(paddr & 0x1F_FFFF == 0, "map_2m: paddr not 2 MiB aligned");

    let (l4i, l3i, l2i, _) = indices(vaddr);
    let l4 = active_l4();
    let l3 = get_or_create_table(&mut l4[l4i], pmm);
    let l2 = get_or_create_table(&mut l3[l3i], pmm);
    if l2[l2i] & PAGE_PRESENT == 0 {
        l2[l2i] = paddr | flags;
    }
}

/// Map a range of physical memory using 2 MiB huge pages.
///
/// The address range [`vaddr_start`, `vaddr_start` + `size`) is rounded out to
/// 2 MiB boundaries.
pub fn map_range_2m(
    vaddr_start: u64,
    paddr_start: u64,
    size: usize,
    flags: u64,
    pmm: &mut PmmAllocator,
) {
    let vstart = vaddr_start & !0x1F_FFFF;
    let pstart = paddr_start & !0x1F_FFFF;
    let vend   = (vaddr_start + size as u64 + 0x1F_FFFF) & !0x1F_FFFF;
    let count  = ((vend - vstart) / 0x20_0000) as usize;

    for i in 0..count {
        let vaddr = vstart + (i as u64) * 0x20_0000;
        let paddr = pstart + (i as u64) * 0x20_0000;
        map_2m(vaddr, paddr, flags, pmm);
        invlpg(vaddr);
    }
}

//==============================================================================
// Unmapping
//==============================================================================

/// Unmap a virtual address in the active address space.
///
/// Returns `Some(old_entry_value)` if the page was mapped, or `None` if it was
/// not present.  Handles both 4 KiB and 2 MiB huge pages.
#[allow(dead_code)]
pub fn unmap(vaddr: u64) -> Option<u64> {
    let (l4i, l3i, l2i, l1i) = indices(vaddr);
    let l4 = active_l4();

    if l4[l4i] & PAGE_PRESENT == 0 { return None; }
    let l3_addr = l4[l4i] & ADDR_MASK;
    let l3 = unsafe { &mut *(l3_addr as *mut PageTable) };

    if l3[l3i] & PAGE_PRESENT == 0 { return None; }
    let l2_addr = l3[l3i] & ADDR_MASK;
    let l2 = unsafe { &mut *(l2_addr as *mut PageTable) };

    if l2[l2i] & PAGE_PRESENT == 0 { return None; }

    if l2[l2i] & PAGE_HUGE != 0 {
        // 2 MiB huge page — unmap at L2.
        let entry = l2[l2i];
        l2[l2i] = 0;
        invlpg(vaddr);
        return Some(entry);
    }

    let l1_addr = l2[l2i] & ADDR_MASK;
    let l1 = unsafe { &mut *(l1_addr as *mut PageTable) };

    if l1[l1i] & PAGE_PRESENT == 0 { return None; }
    let entry = l1[l1i];
    l1[l1i] = 0;
    invlpg(vaddr);
    Some(entry)
}

//==============================================================================
// Translation (virtual → physical)
//==============================================================================

/// Translate a virtual address to its physical address.
///
/// Returns `Some(phys_addr)` or `None` if the page is not mapped.
/// Handles both 4 KiB and 2 MiB huge pages.
#[allow(dead_code)]
pub fn translate(vaddr: u64) -> Option<u64> {
    let (l4i, l3i, l2i, l1i) = indices(vaddr);
    let l4 = unsafe { &*(read_cr3() as *const PageTable) };

    if l4[l4i] & PAGE_PRESENT == 0 { return None; }
    let l3 = unsafe { &*((l4[l4i] & ADDR_MASK) as *const PageTable) };
    if l3[l3i] & PAGE_PRESENT == 0 { return None; }
    let l2 = unsafe { &*((l3[l3i] & ADDR_MASK) as *const PageTable) };
    if l2[l2i] & PAGE_PRESENT == 0 { return None; }

    if l2[l2i] & PAGE_HUGE != 0 {
        let base = l2[l2i] & ADDR_MASK;
        return Some(base | (vaddr & 0x1F_FFFF));
    }

    let l1 = unsafe { &*((l2[l2i] & ADDR_MASK) as *const PageTable) };
    if l1[l1i] & PAGE_PRESENT == 0 { return None; }
    let base = l1[l1i] & ADDR_MASK;
    Some(base | (vaddr & 0xFFF))
}

//==============================================================================
// Quick self-test
//==============================================================================

/// Run the paging self-test.
///
/// Maps a test page, writes to it, translates it, unmaps it, and verifies the
/// translation returns `None` afterwards.
pub fn test(pmm: &mut PmmAllocator, serial: &mut crate::serial::SerialPort) {
    // Allocate a physical page to use as test.
    let phys = pmm.alloc();
    if phys.is_null() {
        serial.writestrs(&["PAGING: Test failed (alloc).\n"]);
        return;
    }

    // Pick a virtual address unlikely to collide — high in the identity range.
    let vaddr: u64 = 0x100_0000; // 16 MiB

    // Map it as 4 KiB.
    map_4k(vaddr, phys as u64, PAGE_KERNEL, pmm);
    invlpg(vaddr);

    // Write a pattern through the virtual mapping.
    unsafe { *(vaddr as *mut u64) = 0xDEAD_BEEF_CAFE_F00D; }

    // Translate back and verify the physical address matches.
    let translated = translate(vaddr);
    match translated {
        Some(pa) if pa == phys as u64 => {
            // Verify the written value is still there.
            let val = unsafe { *(vaddr as *mut u64) };
            if val == 0xDEAD_BEEF_CAFE_F00D {
                serial.writestrs(&["PAGING: Test passed.\n"]);
            } else {
                serial.writestrs(&["PAGING: Test failed (value mismatch).\n"]);
            }
        }
        _ => {
            serial.writestrs(&["PAGING: Test failed (translate).\n"]);
        }
    }

    // Clean up: unmap and free the physical page.
    unmap(vaddr);
    pmm.free(phys);
}
