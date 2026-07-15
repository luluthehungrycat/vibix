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
#[allow(dead_code)]
pub const PAGE_ACCESSED:  u64 = 1 << 5;
#[allow(dead_code)]
pub const PAGE_DIRTY:     u64 = 1 << 6;
pub const PAGE_HUGE:      u64 = 1 << 7;
#[allow(dead_code)]
pub const PAGE_GLOBAL:    u64 = 1 << 8;
#[allow(dead_code)]
pub const PAGE_NO_EXEC:   u64 = 1 << 63;

/// Common flag combinations.
pub const PAGE_KERNEL:       u64 = PAGE_PRESENT | PAGE_WRITABLE;
pub const PAGE_KERNEL_HUGE:  u64 = PAGE_PRESENT | PAGE_WRITABLE | PAGE_HUGE;
pub const PAGE_USER_RW:      u64 = PAGE_PRESENT | PAGE_WRITABLE | PAGE_USER;
#[allow(dead_code)]
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
pub(crate) fn active_l4() -> &'static mut PageTable {
    unsafe { &mut *(read_cr3() as *mut PageTable) }
}
/// Create a new per-process PML4 page table.
///
/// Allocates a fresh PML4, copies kernel mappings from the active PML4, and
/// sets up a per-process PDPT for the identity-mapped low address range
/// (PML4[0]).  User-space PDPT entries are zeroed (not present) unless
/// `copy_user` is true (for fork).
///
/// Returns the physical address of the new PML4.
pub fn create_pml4(pmm: &mut PmmAllocator, copy_user: bool) -> u64 {
    let src_l4 = active_l4();
    let user_code_addr: u64 = 0x2000000; // USER_CODE_ADDR

    // 1. Allocate and zero new PML4 page
    let new_l4_phys = pmm.alloc();
    if new_l4_phys.is_null() {
        loop { unsafe { core::arch::asm!("hlt", options(nomem, nostack)) } }
    }
    let new_l4 = unsafe { &mut *(new_l4_phys as *mut PageTable) };
    for slot in new_l4.iter_mut() { *slot = 0; }

    // 2. Copy PML4 entries from active table.
    //    PML4[0] is special (kernel + user identity map) — needs per-process PDPT.
    //    Other entries (1..511) are kernel-only and can be shared.
    for i in 1..512 {
        new_l4[i] = src_l4[i];
    }

    // 3. Handle PML4[0] — create per-process PDPT with isolated user entries.
    let src_pml4e_0 = src_l4[0];
    if src_pml4e_0 & PAGE_PRESENT != 0 {
        let src_pdpt_phys = src_pml4e_0 & ADDR_MASK;
        let src_pdpt = unsafe { &*(src_pdpt_phys as *const PageTable) };

        // Allocate new PDPT
        let new_pdpt_phys = pmm.alloc();
        if new_pdpt_phys.is_null() {
            loop { unsafe { core::arch::asm!("hlt", options(nomem, nostack)) } }
        }
        let new_pdpt = unsafe { &mut *(new_pdpt_phys as *mut PageTable) };
        for slot in new_pdpt.iter_mut() { *slot = 0; }

        // For each PDPT entry that is present
        for pdpt_idx in 0..512 {
            let src_pdpte = src_pdpt[pdpt_idx];
            if src_pdpte & PAGE_PRESENT == 0 { continue; }

            // If it's a 1 GiB huge page, copy directly (rare in kernel identity map)
            if src_pdpte & PAGE_HUGE != 0 {
                // Check if this huge page covers user addresses
                let base_vaddr = (pdpt_idx as u64) << 30; // 1 GiB per PDPT entry
                if base_vaddr >= user_code_addr && !copy_user {
                    continue; // skip user 1 GiB mapping
                }
                // Preserve flags, strip accessed/dirty for fresh tables
                new_pdpt[pdpt_idx] = src_pdpte & !(PAGE_ACCESSED | PAGE_DIRTY);
                continue;
            }

            // Allocate new PD page for this PDPT entry
            let src_pd_phys = src_pdpte & ADDR_MASK;
            let src_pd = unsafe { &*(src_pd_phys as *const PageTable) };

            let new_pd_phys = pmm.alloc();
            if new_pd_phys.is_null() {
                loop { unsafe { core::arch::asm!("hlt", options(nomem, nostack)) } }
            }
            let new_pd = unsafe { &mut *(new_pd_phys as *mut PageTable) };
            for slot in new_pd.iter_mut() { *slot = 0; }

            // Copy PD entries, skipping user entries if !copy_user
            for pd_idx in 0..512 {
                let src_pde = src_pd[pd_idx];
                if src_pde & PAGE_PRESENT == 0 { continue; }

                // Compute virtual address for this PD entry to check if it's user
                let vaddr = ((pdpt_idx as u64) << 30) | ((pd_idx as u64) << 21);

                if vaddr >= user_code_addr && !copy_user {
                    continue; // skip user mapping
                }

                // For fork (copy_user=true): deep-copy PT tables for user entries
                // to prevent parent/child from sharing the same page table entries.
                // When the child execs an ELF and relaxes page permissions, the
                // parent's pages (flat binary stack at 0x2002000) would also become
                // read-only if they shared the same PT.
                if copy_user && vaddr >= user_code_addr && (src_pde & PAGE_HUGE) == 0 {
                    let src_pt_phys = src_pde & ADDR_MASK;
                    let src_pt = unsafe { &*(src_pt_phys as *const PageTable) };
                    let new_pt_phys = pmm.alloc();
                    if new_pt_phys.is_null() {
                        loop { unsafe { core::arch::asm!("hlt", options(nomem, nostack)) } }
                    }
                    let new_pt = unsafe { &mut *(new_pt_phys as *mut PageTable) };
                    for slot in new_pt.iter_mut() { *slot = 0; }
                    for k in 0..512 {
                        new_pt[k] = src_pt[k];
                    }
                    let pt_flags = src_pde & !ADDR_MASK;
                    new_pd[pd_idx] = (new_pt_phys as u64) | pt_flags;
                } else {
                    new_pd[pd_idx] = src_pde;
                }
            }

            // Wire the new PD into the new PDPT
            // Strip accessed/dirty from intermediate entry for fresh tables
            let flags = src_pdpte & (PAGE_PRESENT | PAGE_WRITABLE | PAGE_USER | PAGE_ACCESSED | PAGE_DIRTY | PAGE_HUGE | PAGE_GLOBAL | PAGE_NO_EXEC);
            new_pdpt[pdpt_idx] = (new_pd_phys as u64) | (flags & !(PAGE_ACCESSED | PAGE_DIRTY | PAGE_HUGE));
        }

        // Wire the new PDPT into the new PML4[0]
        let flags = src_pml4e_0 & (PAGE_PRESENT | PAGE_WRITABLE | PAGE_USER | PAGE_ACCESSED | PAGE_DIRTY | PAGE_HUGE | PAGE_GLOBAL | PAGE_NO_EXEC);
        new_l4[0] = (new_pdpt_phys as u64) | (flags & !(PAGE_ACCESSED | PAGE_DIRTY | PAGE_HUGE));
    }

    new_l4_phys as u64
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

/// Map a single 4 KiB page in a target process's address space.
///
/// Temporarily switches CR3 to `target_pml4_phys`, maps the page via `map_4k`,
/// flushes the TLB entry (while target CR3 is active), then restores the
/// original CR3.
///
/// Both `vaddr` and `paddr` must be 4 KiB aligned.
pub fn map_4k_target(vaddr: u64, paddr: u64, flags: u64, pmm: &mut PmmAllocator, target_pml4_phys: u64) {
    let saved_cr3 = read_cr3();
    if target_pml4_phys != saved_cr3 {
        unsafe { write_cr3(target_pml4_phys); }
    }
    map_4k(vaddr, paddr, flags, pmm);
    invlpg(vaddr); // flush TLB for this vaddr while target CR3 is active
    if target_pml4_phys != saved_cr3 {
        unsafe { write_cr3(saved_cr3); }
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

/// Translate a virtual address in a SPECIFIC PML4 table.
/// This does NOT use `read_cr3()` — it walks the given PML4 directly.
/// Useful for checking if a page is already mapped in a target process's
/// page tables during ELF loading, without having to switch CR3.
pub fn translate_in_pml4(vaddr: u64, pml4_phys: u64) -> Option<u64> {
    let (l4i, l3i, l2i, l1i) = indices(vaddr);
    let l4 = unsafe { &*(pml4_phys as *const PageTable) };

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
