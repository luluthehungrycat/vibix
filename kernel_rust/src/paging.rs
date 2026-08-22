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

pub const PAGE_PRESENT: u64 = 1 << 0;
pub const PAGE_WRITABLE: u64 = 1 << 1;
pub const PAGE_USER: u64 = 1 << 2;
#[allow(dead_code)]
pub const PAGE_ACCESSED: u64 = 1 << 5;
#[allow(dead_code)]
pub const PAGE_DIRTY: u64 = 1 << 6;
pub const PAGE_HUGE: u64 = 1 << 7;
#[allow(dead_code)]
pub const PAGE_GLOBAL: u64 = 1 << 8;
#[allow(dead_code)]
pub const PAGE_NO_EXEC: u64 = 1 << 63;

/// Common flag combinations.
pub const PAGE_KERNEL: u64 = PAGE_PRESENT | PAGE_WRITABLE;
pub const PAGE_KERNEL_HUGE: u64 = PAGE_PRESENT | PAGE_WRITABLE | PAGE_HUGE;
pub const PAGE_USER_RW: u64 = PAGE_PRESENT | PAGE_WRITABLE | PAGE_USER;
#[allow(dead_code)]
pub const PAGE_USER_RO: u64 = PAGE_PRESENT | PAGE_USER;

/// Physical address mask (bits 12–47).
const ADDR_MASK: u64 = 0x000FFFFF_FFFFF000;

/// Kernel-only temporary mapping used when copying a forked user leaf.
///
/// This address is outside the supported process ranges (`BRK_MAX` and the
/// ELF user limit) and is never installed with `PAGE_USER`.  Its intermediate
/// tables are provisioned transactionally before a child leaf is copied.
pub const KERNEL_SCRATCH_VA: u64 = 0x0000_4000_0000;

/// Return whether a page-aligned range overlaps the reserved scratch page.
pub const fn overlaps_kernel_scratch(start: u64, end: u64) -> bool {
    start < KERNEL_SCRATCH_VA + 0x1000 && end > KERNEL_SCRATCH_VA
}

/// A 512-entry page table (all levels: PML4, PDPT, PD, PT).
pub type PageTable = [u64; 512];

//==============================================================================
// CPU register access
//==============================================================================

/// Read CR3 (physical address of current PML4), with lower 12 bits masked off.
pub fn read_cr3() -> u64 {
    let cr3: u64;
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) cr3);
    }
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
    unsafe {
        core::arch::asm!("invlpg [{v}]", v = in(reg) vaddr, options(nostack, preserves_flags));
    }
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
        loop {
            unsafe { core::arch::asm!("hlt", options(nomem, nostack)) }
        }
    }
    let table = unsafe { &mut *(page as *mut PageTable) };
    for slot in table.iter_mut() {
        *slot = 0;
    }
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

/// Keep the page-table transaction atomic with respect to interrupt handlers.
/// The guard restores the caller's IF state rather than unconditionally
/// enabling interrupts on exit.
struct InterruptGuard(u64);

impl InterruptGuard {
    fn disable() -> Self {
        let flags: u64;
        unsafe {
            core::arch::asm!("pushfq; pop {}", out(reg) flags, options(nomem));
            core::arch::asm!("cli", options(nomem, nostack));
        }
        Self(flags)
    }
}

impl Drop for InterruptGuard {
    fn drop(&mut self) {
        if self.0 & 0x200 != 0 {
            unsafe {
                core::arch::asm!("sti", options(nomem, nostack));
            }
        }
    }
}

fn zero_table(page: *mut u8) -> *mut PageTable {
    let table = page as *mut PageTable;
    unsafe {
        for slot in (*table).iter_mut() {
            *slot = 0;
        }
    }
    table
}

#[derive(Clone, Copy)]
struct ScratchProvision {
    /// Newly allocated PD and PT frames under the active root.  Zero means the
    /// corresponding branch pre-existed and is borrowed, never reclaimed.
    new_pd: u64,
    new_pt: u64,
}

/// Provision the kernel scratch walk without leaving partially-created
/// branches when any PMM allocation fails. Existing kernel branches are
/// borrowed and never reclaimed; the returned ownership record lets callers
/// remove only branches created by this fork transaction.
fn provision_kernel_scratch(pmm: &mut PmmAllocator) -> Option<ScratchProvision> {
    let _irq = InterruptGuard::disable();
    let (l4i, l3i, l2i, l1i) = indices(KERNEL_SCRATCH_VA);
    let mut new_pd = 0u64;
    let mut new_pt = 0u64;

    unsafe {
        let l4 = active_l4() as *mut PageTable;
        if (*l4)[l4i] & PAGE_PRESENT == 0 || (*l4)[l4i] & PAGE_HUGE != 0 {
            return None;
        }
        let l3 = ((*l4)[l4i] & ADDR_MASK) as *mut PageTable;
        if (*l3)[l3i] & PAGE_PRESENT == 0 {
            let page = pmm.alloc();
            if page.is_null() {
                return None;
            }
            zero_table(page);
            (*l3)[l3i] = page as u64 | PAGE_KERNEL;
            new_pd = page as u64;
        } else if (*l3)[l3i] & PAGE_HUGE != 0 {
            return None;
        }

        let l2 = ((*l3)[l3i] & ADDR_MASK) as *mut PageTable;
        if (*l2)[l2i] & PAGE_PRESENT == 0 {
            let page = pmm.alloc();
            if page.is_null() {
                if new_pd != 0 {
                    (*l3)[l3i] = 0;
                    pmm.free(new_pd as *mut u8);
                }
                return None;
            }
            zero_table(page);
            (*l2)[l2i] = page as u64 | PAGE_KERNEL;
            new_pt = page as u64;
        } else if (*l2)[l2i] & PAGE_HUGE != 0 {
            if new_pd != 0 {
                (*l3)[l3i] = 0;
                pmm.free(new_pd as *mut u8);
            }
            return None;
        }

        let pt = ((*l2)[l2i] & ADDR_MASK) as *mut PageTable;
        if (*pt)[l1i] & PAGE_PRESENT != 0 || (*pt)[l1i] != 0 {
            if new_pt != 0 {
                (*l2)[l2i] = 0;
                pmm.free(new_pt as *mut u8);
            }
            if new_pd != 0 {
                (*l3)[l3i] = 0;
                pmm.free(new_pd as *mut u8);
            }
            return None;
        }
    }
    Some(ScratchProvision { new_pd, new_pt })
}

/// Remove only the scratch branches allocated by `provision_kernel_scratch`.
/// The scratch leaf must already be unmapped; pre-existing branches are left
/// untouched even if they are empty.
fn cleanup_kernel_scratch(pmm: &mut PmmAllocator, provision: ScratchProvision) {
    if provision.new_pd == 0 && provision.new_pt == 0 {
        return;
    }
    let _irq = InterruptGuard::disable();
    let (l4i, l3i, l2i, _l1i) = indices(KERNEL_SCRATCH_VA);
    unsafe {
        let l4 = active_l4() as *mut PageTable;
        if (*l4)[l4i] & PAGE_PRESENT == 0 || (*l4)[l4i] & PAGE_HUGE != 0 {
            return;
        }
        let l3 = ((*l4)[l4i] & ADDR_MASK) as *mut PageTable;
        if (*l3)[l3i] & PAGE_PRESENT == 0 || (*l3)[l3i] & PAGE_HUGE != 0 {
            return;
        }
        let l2 = ((*l3)[l3i] & ADDR_MASK) as *mut PageTable;

        if provision.new_pt != 0
            && (*l2)[l2i] & ADDR_MASK == provision.new_pt
            && (*l2)[l2i] & PAGE_HUGE == 0
        {
            let pt = ((*l2)[l2i] & ADDR_MASK) as *mut PageTable;
            if (*pt).iter().all(|entry| *entry == 0) {
                (*l2)[l2i] = 0;
                pmm.free(provision.new_pt as *mut u8);
            }
        }

        if provision.new_pd != 0
            && (*l3)[l3i] & ADDR_MASK == provision.new_pd
            && (*l3)[l3i] & PAGE_HUGE == 0
        {
            let pd = ((*l3)[l3i] & ADDR_MASK) as *mut PageTable;
            if (*pd).iter().all(|entry| *entry == 0) {
                (*l3)[l3i] = 0;
                pmm.free(provision.new_pd as *mut u8);
            }
        }
    }
}

fn kernel_scratch_pte() -> Option<*mut u64> {
    let (l4i, l3i, l2i, l1i) = indices(KERNEL_SCRATCH_VA);
    unsafe {
        let l4 = active_l4() as *mut PageTable;
        if (*l4)[l4i] & PAGE_PRESENT == 0 || (*l4)[l4i] & PAGE_HUGE != 0 {
            return None;
        }
        let l3 = ((*l4)[l4i] & ADDR_MASK) as *mut PageTable;
        if (*l3)[l3i] & PAGE_PRESENT == 0 || (*l3)[l3i] & PAGE_HUGE != 0 {
            return None;
        }
        let l2 = ((*l3)[l3i] & ADDR_MASK) as *mut PageTable;
        if (*l2)[l2i] & PAGE_PRESENT == 0 || (*l2)[l2i] & PAGE_HUGE != 0 {
            return None;
        }
        let l1 = ((*l2)[l2i] & ADDR_MASK) as *mut PageTable;
        Some(&mut (*l1)[l1i])
    }
}

fn copy_user_page_through_scratch(src_vaddr: u64, child_page: *mut u8) -> bool {
    let _irq = InterruptGuard::disable();
    let pte = match kernel_scratch_pte() {
        Some(pte) => pte,
        None => return false,
    };
    unsafe {
        if *pte & PAGE_PRESENT != 0 {
            return false;
        }
        *pte = child_page as u64 | PAGE_KERNEL;
        invlpg(KERNEL_SCRATCH_VA);
        core::ptr::copy_nonoverlapping(
            src_vaddr as *const u8,
            KERNEL_SCRATCH_VA as *mut u8,
            0x1000,
        );
        *pte = 0;
        invlpg(KERNEL_SCRATCH_VA);
    }
    true
}

/// A newly allocated address-space root and its ownership mode.
///
/// `owns_user_leaves` is true for image roots and eagerly-cloned fork roots.
/// A root with this bit set owns every present user 4 KiB leaf beneath its
/// PML4[0] tree. Fork user leaves are child-owned eager copies, never borrowed
/// parent frames, and may be reclaimed during failed construction.
#[derive(Clone, Copy)]
pub struct Pml4Handle {
    pub phys: u64,
    pub owns_user_leaves: bool,
}

/// Create a new per-process PML4 page table.
///
/// Allocates a fresh PML4, copies kernel mappings from the active PML4, and
/// sets up a per-process PDPT for the identity-mapped low address range
/// (PML4[0]).  User-space PDPT entries are zeroed (not present) unless
/// `copy_user` is true (for fork). Fork construction eagerly clones every
/// present user 4 KiB leaf, so parent and child never share writable frames.
///
/// Returns `None` when any page-table allocation fails.  Partial constructor
/// state is reclaimed before returning, and only constructor-owned tables are
/// freed.
pub fn create_pml4(pmm: &mut PmmAllocator, copy_user: bool) -> Option<Pml4Handle> {
    create_pml4_inner(pmm, copy_user, None)
}

/// DEBUG-only constructor hook used by the fork-isolation regression. The
/// injected failure occurs before the Nth cloned user leaf allocation.
#[cfg(feature = "debug")]
pub fn create_pml4_with_clone_failure(
    pmm: &mut PmmAllocator,
    copy_user: bool,
    fail_after_clones: usize,
) -> Option<Pml4Handle> {
    create_pml4_inner(pmm, copy_user, Some(fail_after_clones))
}

fn create_pml4_inner(
    pmm: &mut PmmAllocator,
    copy_user: bool,
    fail_after_clones: Option<usize>,
) -> Option<Pml4Handle> {
    // Fork copies are performed through a reserved kernel-only mapping in the
    // active (parent) root.  Provision its page-table branch before allocating
    // any child state so a PMM failure cannot leave a partial scratch walk.
    let scratch_provision = if copy_user {
        match provision_kernel_scratch(pmm) {
            Some(provision) => Some(provision),
            None => return None,
        }
    } else {
        None
    };
    macro_rules! fail_construction {
        () => {{
            if let Some(provision) = scratch_provision {
                cleanup_kernel_scratch(pmm, provision);
            }
            return None;
        }};
    }
    let src_l4 = active_l4();
    let user_code_addr: u64 = 0x2000000; // USER_CODE_ADDR
    let mut cloned_leaves = 0usize;

    // 1. Allocate and zero new PML4 page
    let new_l4_phys = pmm.alloc();
    if new_l4_phys.is_null() {
        fail_construction!();
    }
    let new_l4 = unsafe { &mut *(new_l4_phys as *mut PageTable) };
    for slot in new_l4.iter_mut() {
        *slot = 0;
    }

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
            destroy_uncommitted_pml4(
                Pml4Handle {
                    phys: new_l4_phys as u64,
                    owns_user_leaves: true,
                },
                pmm,
            );
            fail_construction!();
        }
        let new_pdpt = unsafe { &mut *(new_pdpt_phys as *mut PageTable) };
        for slot in new_pdpt.iter_mut() {
            *slot = 0;
        }
        let flags = src_pml4e_0
            & (PAGE_PRESENT
                | PAGE_WRITABLE
                | PAGE_USER
                | PAGE_ACCESSED
                | PAGE_DIRTY
                | PAGE_HUGE
                | PAGE_GLOBAL
                | PAGE_NO_EXEC);
        // Publish the owned PDPT immediately so constructor cleanup can find
        // it even if a later PD/PT allocation fails.
        new_l4[0] = (new_pdpt_phys as u64) | (flags & !(PAGE_ACCESSED | PAGE_DIRTY | PAGE_HUGE));

        // For each PDPT entry that is present
        for pdpt_idx in 0..512 {
            let src_pdpte = src_pdpt[pdpt_idx];
            if src_pdpte & PAGE_PRESENT == 0 {
                continue;
            }

            let base_vaddr = (pdpt_idx as u64) << 30;
            if overlaps_kernel_scratch(base_vaddr, base_vaddr + (1u64 << 30)) {
                continue;
            }

            // If it's a 1 GiB huge page, copy kernel-only mappings directly.
            if src_pdpte & PAGE_HUGE != 0 {
                let huge_end = base_vaddr.saturating_add(1u64 << 30);
                if copy_user && src_pdpte & PAGE_USER != 0 && huge_end > user_code_addr {
                    destroy_uncommitted_pml4(
                        Pml4Handle {
                            phys: new_l4_phys as u64,
                            owns_user_leaves: true,
                        },
                        pmm,
                    );
                    fail_construction!();
                }
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
                destroy_uncommitted_pml4(
                    Pml4Handle {
                        phys: new_l4_phys as u64,
                        owns_user_leaves: true,
                    },
                    pmm,
                );
                fail_construction!();
            }
            let new_pd = unsafe { &mut *(new_pd_phys as *mut PageTable) };
            for slot in new_pd.iter_mut() {
                *slot = 0;
            }
            let flags = src_pdpte
                & (PAGE_PRESENT
                    | PAGE_WRITABLE
                    | PAGE_USER
                    | PAGE_ACCESSED
                    | PAGE_DIRTY
                    | PAGE_HUGE
                    | PAGE_GLOBAL
                    | PAGE_NO_EXEC);
            // Publish the owned PD before copying user PTs so a failed PT
            // allocation is covered by the same constructor cleanup path.
            new_pdpt[pdpt_idx] =
                (new_pd_phys as u64) | (flags & !(PAGE_ACCESSED | PAGE_DIRTY | PAGE_HUGE));

            // Copy PD entries, skipping user entries if !copy_user
            for pd_idx in 0..512 {
                let src_pde = src_pd[pd_idx];
                if src_pde & PAGE_PRESENT == 0 {
                    continue;
                }

                // Compute virtual address for this PD entry to check if it's user
                let vaddr = ((pdpt_idx as u64) << 30) | ((pd_idx as u64) << 21);

                if copy_user
                    && src_pde & PAGE_HUGE != 0
                    && src_pde & PAGE_USER != 0
                    && vaddr.saturating_add(1u64 << 21) > user_code_addr
                {
                    destroy_uncommitted_pml4(
                        Pml4Handle {
                            phys: new_l4_phys as u64,
                            owns_user_leaves: true,
                        },
                        pmm,
                    );
                    fail_construction!();
                }

                if vaddr >= user_code_addr && !copy_user {
                    continue; // skip user mapping
                }

                // For fork (copy_user=true): deep-copy PT tables for user entries
                // to prevent parent/child from sharing the same page table entries.
                // When the child execs an ELF and relaxes page permissions, the
                // parent's pages (including a flat binary's dynamically placed
                // stack) would also become read-only if they shared the same PT.
                if copy_user && vaddr >= user_code_addr && (src_pde & PAGE_HUGE) == 0 {
                    let src_pt_phys = src_pde & ADDR_MASK;
                    let src_pt = unsafe { &*(src_pt_phys as *const PageTable) };
                    let new_pt_phys = pmm.alloc();
                    if new_pt_phys.is_null() {
                        destroy_uncommitted_pml4(
                            Pml4Handle {
                                phys: new_l4_phys as u64,
                                owns_user_leaves: true,
                            },
                            pmm,
                        );
                        fail_construction!();
                    }
                    let new_pt = unsafe { &mut *(new_pt_phys as *mut PageTable) };
                    for slot in new_pt.iter_mut() {
                        *slot = 0;
                    }
                    // Publish the private PT table before filling it so the
                    // constructor's rollback walk owns and can reclaim any
                    // child leaves already copied into it. Individual child
                    // PTEs remain unpublished until their copy succeeds.
                    let pt_flags = src_pde & !ADDR_MASK;
                    new_pd[pd_idx] = (new_pt_phys as u64) | pt_flags;
                    for k in 0..512 {
                        let src_pte = src_pt[k];
                        if src_pte & PAGE_PRESENT == 0 || src_pte & PAGE_USER == 0 {
                            new_pt[k] = src_pte;
                            continue;
                        }
                        if fail_after_clones == Some(cloned_leaves) {
                            destroy_uncommitted_pml4(
                                Pml4Handle {
                                    phys: new_l4_phys as u64,
                                    owns_user_leaves: true,
                                },
                                pmm,
                            );
                            fail_construction!();
                        }
                        let child_leaf = pmm.alloc();
                        if child_leaf.is_null() {
                            destroy_uncommitted_pml4(
                                Pml4Handle {
                                    phys: new_l4_phys as u64,
                                    owns_user_leaves: true,
                                },
                                pmm,
                            );
                            fail_construction!();
                        }
                        let src_vaddr = vaddr + (k as u64) * 0x1000;
                        if !copy_user_page_through_scratch(src_vaddr, child_leaf) {
                            pmm.free(child_leaf);
                            destroy_uncommitted_pml4(
                                Pml4Handle {
                                    phys: new_l4_phys as u64,
                                    owns_user_leaves: true,
                                },
                                pmm,
                            );
                            fail_construction!();
                        }
                        // Publish only after the private frame is copied.
                        new_pt[k] = child_leaf as u64 | (src_pte & !ADDR_MASK);
                        cloned_leaves += 1;
                    }
                } else if copy_user && vaddr >= user_code_addr && (src_pde & PAGE_HUGE) != 0 {
                    // A user 2 MiB mapping has no safe eager 4 KiB clone path.
                    if src_pde & PAGE_USER != 0 {
                        destroy_uncommitted_pml4(
                            Pml4Handle {
                                phys: new_l4_phys as u64,
                                owns_user_leaves: true,
                            },
                            pmm,
                        );
                        fail_construction!();
                    }
                    new_pd[pd_idx] = src_pde;
                } else {
                    new_pd[pd_idx] = src_pde;
                }
            }

        }
    }

    if let Some(provision) = scratch_provision {
        cleanup_kernel_scratch(pmm, provision);
    }
    Some(Pml4Handle {
        phys: new_l4_phys as u64,
        owns_user_leaves: true,
    })
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
    if flags & PAGE_USER != 0
        && overlaps_kernel_scratch(vaddr, vaddr.saturating_add(0x1000))
    {
        return;
    }

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
            if cfg!(feature = "debug") && l4i == 0 && vaddr == 0x2000000 {
                use core::fmt::Write;
                let mut ser = crate::serial::SerialPort::new();
                ser.init();
                let _ = write!(
                    ser,
                    "DBG map_4k: vaddr={:016x} l4[0]={:016x}",
                    vaddr,
                    (*l4)[0]
                );
            }
            (*l4)[l4i] |= PAGE_USER;
            (*l3_ptr)[l3i] |= PAGE_USER;
            (*l2_ptr)[l2i] |= PAGE_USER;
            if cfg!(feature = "debug") && l4i == 0 && vaddr == 0x2000000 {
                use core::fmt::Write;
                let mut ser = crate::serial::SerialPort::new();
                ser.init();
                let _ = write!(
                    ser,
                    " -> {:016x}
",
                    (*l4)[0]
                );
            }
        }
    }
}

/// Fallible 4 KiB mapping used by transactional loaders.  Unlike
/// `map_4k`, this path reports an intermediate page-table allocation failure
/// instead of halting, so the caller can discard the incomplete image.
fn try_map_4k(vaddr: u64, paddr: u64, flags: u64, pmm: &mut PmmAllocator) -> bool {
    debug_assert!(vaddr & 0xFFF == 0, "try_map_4k: vaddr not page-aligned");
    debug_assert!(paddr & 0xFFF == 0, "try_map_4k: paddr not page-aligned");
    if flags & PAGE_USER != 0
        && overlaps_kernel_scratch(vaddr, vaddr.saturating_add(0x1000))
    {
        return false;
    }

    // Transactional callers must never replace an existing leaf.  Checking
    // before creating intermediate tables also avoids leaking empty branches
    // on a collision.
    if translate_in_pml4(vaddr, read_cr3()).is_some() {
        return false;
    }

    let (l4i, l3i, l2i, l1i) = indices(vaddr);
    unsafe {
        let l4 = active_l4() as *mut PageTable;
        let l3_ptr = match try_get_or_create_table(&mut (*l4)[l4i], pmm) {
            Some(table) => table,
            None => return false,
        };
        let l2_ptr = match try_get_or_create_table(&mut (*l3_ptr)[l3i], pmm) {
            Some(table) => table,
            None => return false,
        };
        let l1_ptr = match try_get_or_create_table(&mut (*l2_ptr)[l2i], pmm) {
            Some(table) => table,
            None => return false,
        };
        (*l1_ptr)[l1i] = paddr | flags;
        if flags & PAGE_USER != 0 {
            (*l4)[l4i] |= PAGE_USER;
            (*l3_ptr)[l3i] |= PAGE_USER;
            (*l2_ptr)[l2i] |= PAGE_USER;
        }
    }
    true
}

fn try_get_or_create_table(entry: &mut u64, pmm: &mut PmmAllocator) -> Option<*mut PageTable> {
    if *entry & PAGE_PRESENT != 0 {
        return Some((*entry & ADDR_MASK) as *mut PageTable);
    }
    let page = pmm.alloc();
    if page.is_null() {
        return None;
    }
    let table = page as *mut PageTable;
    unsafe {
        for slot in (*table).iter_mut() {
            *slot = 0;
        }
    }
    *entry = page as u64 | PAGE_KERNEL | PAGE_USER;
    Some(table)
}

/// Map a single 4 KiB page in a target process's address space.
///
/// Temporarily switches CR3 to `target_pml4_phys`, maps the page via `map_4k`,
/// flushes the TLB entry (while target CR3 is active), then restores the
/// original CR3.
///
/// Both `vaddr` and `paddr` must be 4 KiB aligned.
pub fn map_4k_target(
    vaddr: u64,
    paddr: u64,
    flags: u64,
    pmm: &mut PmmAllocator,
    target_pml4_phys: u64,
) {
    let saved_cr3 = read_cr3();
    if target_pml4_phys != saved_cr3 {
        unsafe {
            write_cr3(target_pml4_phys);
        }
    }
    map_4k(vaddr, paddr, flags, pmm);
    invlpg(vaddr); // flush TLB for this vaddr while target CR3 is active
    if target_pml4_phys != saved_cr3 {
        unsafe {
            write_cr3(saved_cr3);
        }
    }
}

/// Fallible target mapping for operations that must roll back on OOM.
pub fn try_map_4k_target(
    vaddr: u64,
    paddr: u64,
    flags: u64,
    pmm: &mut PmmAllocator,
    target_pml4_phys: u64,
) -> bool {
    let saved_cr3 = read_cr3();
    if target_pml4_phys != saved_cr3 {
        unsafe {
            write_cr3(target_pml4_phys);
        }
    }
    let mapped = try_map_4k(vaddr, paddr, flags, pmm);
    if mapped {
        invlpg(vaddr);
    }
    if target_pml4_phys != saved_cr3 {
        unsafe {
            write_cr3(saved_cr3);
        }
    }
    mapped
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
    let vend = (vaddr_start + size as u64 + 0x1F_FFFF) & !0x1F_FFFF;
    let count = ((vend - vstart) / 0x20_0000) as usize;

    for i in 0..count {
        let vaddr = vstart + (i as u64) * 0x20_0000;
        let paddr = pstart + (i as u64) * 0x20_0000;
        map_2m(vaddr, paddr, flags, pmm);
        invlpg(vaddr);
    }
}

/// Extend the kernel-only low identity map without covering USER_CODE_ADDR.
pub fn extend_kernel_identity(size: usize, pmm: &mut PmmAllocator) {
    map_range_2m(0, 0, size, PAGE_KERNEL_HUGE, pmm);
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

    if l4[l4i] & PAGE_PRESENT == 0 {
        return None;
    }
    let l3_addr = l4[l4i] & ADDR_MASK;
    let l3 = unsafe { &mut *(l3_addr as *mut PageTable) };

    if l3[l3i] & PAGE_PRESENT == 0 {
        return None;
    }
    let l2_addr = l3[l3i] & ADDR_MASK;
    let l2 = unsafe { &mut *(l2_addr as *mut PageTable) };

    if l2[l2i] & PAGE_PRESENT == 0 {
        return None;
    }

    if l2[l2i] & PAGE_HUGE != 0 {
        // 2 MiB huge page — unmap at L2.
        let entry = l2[l2i];
        l2[l2i] = 0;
        invlpg(vaddr);
        return Some(entry);
    }

    let l1_addr = l2[l2i] & ADDR_MASK;
    let l1 = unsafe { &mut *(l1_addr as *mut PageTable) };

    if l1[l1i] & PAGE_PRESENT == 0 {
        return None;
    }
    let entry = l1[l1i];
    l1[l1i] = 0;
    invlpg(vaddr);
    Some(entry)
}

/// Unmap a leaf and reclaim empty page-table levels created for it.
///
/// This is used only for loader-owned rollback.  A non-empty parent table is
/// retained, which preserves inherited mappings in an existing process table.
pub fn unmap_and_free_empty(vaddr: u64, pmm: &mut PmmAllocator) -> Option<u64> {
    let (l4i, l3i, l2i, l1i) = indices(vaddr);
    let l4 = active_l4();
    if l4[l4i] & PAGE_PRESENT == 0 {
        return None;
    }
    let l3_addr = l4[l4i] & ADDR_MASK;
    let l3 = unsafe { &mut *(l3_addr as *mut PageTable) };
    if l3[l3i] & PAGE_PRESENT == 0 {
        return None;
    }
    let l2_addr = l3[l3i] & ADDR_MASK;
    let l2 = unsafe { &mut *(l2_addr as *mut PageTable) };
    if l2[l2i] & PAGE_PRESENT == 0 || l2[l2i] & PAGE_HUGE != 0 {
        return unmap(vaddr);
    }
    let l1_addr = l2[l2i] & ADDR_MASK;
    let l1 = unsafe { &mut *(l1_addr as *mut PageTable) };
    if l1[l1i] & PAGE_PRESENT == 0 {
        return None;
    }
    let entry = l1[l1i];
    l1[l1i] = 0;
    invlpg(vaddr);

    if l1.iter().all(|slot| *slot == 0) {
        l2[l2i] = 0;
        pmm.free(l1_addr as *mut u8);
        if l2.iter().all(|slot| *slot == 0) {
            l3[l3i] = 0;
            pmm.free(l2_addr as *mut u8);
            if l3.iter().all(|slot| *slot == 0) {
                l4[l4i] = 0;
                pmm.free(l3_addr as *mut u8);
            }
        }
    }
    Some(entry)
}

/// Destroy a fresh per-process page table that was never committed.
///
/// The low kernel tables are shared with the active address space and are not
/// freed. Every PDPT/PD/PT page under PML4[0] was allocated by the constructor,
/// so those table pages are always reclaimed. User leaf frames are reclaimed
/// for both image roots and fork roots because fork leaves are child-owned
/// eager copies; parent frames are never touched.
pub fn destroy_uncommitted_pml4(handle: Pml4Handle, pmm: &mut PmmAllocator) {
    const USER_CODE_ADDR: u64 = 0x0200_0000;
    let l4 = unsafe { &mut *(handle.phys as *mut PageTable) };
    if l4[0] & PAGE_PRESENT == 0 {
        pmm.free(handle.phys as *mut u8);
        return;
    }
    let pdpt_phys = l4[0] & ADDR_MASK;
    let pdpt = unsafe { &mut *(pdpt_phys as *mut PageTable) };
    for pdpt_idx in 0..512 {
        let pdpte = pdpt[pdpt_idx];
        if pdpte & PAGE_PRESENT == 0 || pdpte & PAGE_HUGE != 0 {
            continue;
        }
        let pd_phys = pdpte & ADDR_MASK;
        let pd = unsafe { &mut *(pd_phys as *mut PageTable) };
        for pd_idx in 0..512 {
            let vaddr = ((pdpt_idx as u64) << 30) | ((pd_idx as u64) << 21);
            let pde = pd[pd_idx];
            if pde & PAGE_PRESENT == 0 {
                continue;
            }
            if vaddr < USER_CODE_ADDR {
                continue;
            }
            if pde & PAGE_HUGE != 0 {
                if handle.owns_user_leaves && pde & PAGE_USER != 0 {
                    pmm.free((pde & ADDR_MASK) as *mut u8);
                }
                continue;
            }
            let pt_phys = pde & ADDR_MASK;
            let pt = unsafe { &mut *(pt_phys as *mut PageTable) };
            for pte in pt.iter() {
                if handle.owns_user_leaves && *pte & PAGE_PRESENT != 0 && *pte & PAGE_USER != 0 {
                    pmm.free((*pte & ADDR_MASK) as *mut u8);
                }
            }
            pmm.free(pt_phys as *mut u8);
        }
        pmm.free(pd_phys as *mut u8);
    }
    pmm.free(pdpt_phys as *mut u8);
    pmm.free(handle.phys as *mut u8);
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

    if l4[l4i] & PAGE_PRESENT == 0 {
        return None;
    }
    let l3 = unsafe { &*((l4[l4i] & ADDR_MASK) as *const PageTable) };
    if l3[l3i] & PAGE_PRESENT == 0 {
        return None;
    }
    let l2 = unsafe { &*((l3[l3i] & ADDR_MASK) as *const PageTable) };
    if l2[l2i] & PAGE_PRESENT == 0 {
        return None;
    }

    if l2[l2i] & PAGE_HUGE != 0 {
        let base = l2[l2i] & ADDR_MASK;
        return Some(base | (vaddr & 0x1F_FFFF));
    }

    let l1 = unsafe { &*((l2[l2i] & ADDR_MASK) as *const PageTable) };
    if l1[l1i] & PAGE_PRESENT == 0 {
        return None;
    }
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

    if l4[l4i] & PAGE_PRESENT == 0 {
        return None;
    }
    let l3 = unsafe { &*((l4[l4i] & ADDR_MASK) as *const PageTable) };
    if l3[l3i] & PAGE_PRESENT == 0 {
        return None;
    }
    let l2 = unsafe { &*((l3[l3i] & ADDR_MASK) as *const PageTable) };
    if l2[l2i] & PAGE_PRESENT == 0 {
        return None;
    }

    if l2[l2i] & PAGE_HUGE != 0 {
        let base = l2[l2i] & ADDR_MASK;
        return Some(base | (vaddr & 0x1F_FFFF));
    }

    let l1 = unsafe { &*((l2[l2i] & ADDR_MASK) as *const PageTable) };
    if l1[l1i] & PAGE_PRESENT == 0 {
        return None;
    }
    let base = l1[l1i] & ADDR_MASK;
    Some(base | (vaddr & 0xFFF))
}

/// DEBUG-only page-table and entry-page evidence helpers.
///
/// Physical bytes are read only after verifying that the active root identity-maps
/// the requested physical page.  This keeps the diagnostic from accidentally
/// treating an arbitrary physical address as a kernel virtual pointer.
pub fn debug_dump_walk(label: &str, vaddr: u64, pml4_phys: u64) {
    if !cfg!(feature = "debug") {
        return;
    }
    debug_dump_root_walk(label, vaddr, pml4_phys);
}

pub fn debug_dump_root_walk(label: &str, vaddr: u64, pml4_phys: u64) {
    if !cfg!(feature = "debug") {
        return;
    }
    use core::fmt::Write;
    let mut serial = crate::serial::SerialPort::new();
    serial.init();
    let active = read_cr3();
    let (l4i, l3i, l2i, l1i) = indices(vaddr);
    let l4 = unsafe { &*(pml4_phys as *const PageTable) };
    let l4e = l4[l4i];
    let _ = write!(serial,
        "DBG ELF WALK: {} root=0x{:016x} active_cr3=0x{:016x} vaddr=0x{:016x} idx={}/{}/{}/{} PML4E=0x{:016x}\n",
        label, pml4_phys, active, vaddr, l4i, l3i, l2i, l1i, l4e);
    if l4e & PAGE_PRESENT == 0 {
        return;
    }
    let l3 = unsafe { &*((l4e & ADDR_MASK) as *const PageTable) };
    let l3e = l3[l3i];
    let _ = write!(serial, "DBG ELF WALK: {} PDPTE=0x{:016x}\n", label, l3e);
    if l3e & PAGE_PRESENT == 0 {
        return;
    }
    if l3e & PAGE_HUGE != 0 {
        let phys = (l3e & ADDR_MASK) | (vaddr & 0x3fff_ffff);
        debug_dump_physical_if_identity(&mut serial, label, phys);
        return;
    }
    let l2 = unsafe { &*((l3e & ADDR_MASK) as *const PageTable) };
    let l2e = l2[l2i];
    let _ = write!(serial, "DBG ELF WALK: {} PDE=0x{:016x}\n", label, l2e);
    if l2e & PAGE_PRESENT == 0 {
        return;
    }
    if l2e & PAGE_HUGE != 0 {
        let phys = (l2e & ADDR_MASK) | (vaddr & 0x1f_ffff);
        debug_dump_physical_if_identity(&mut serial, label, phys);
        return;
    }
    let l1 = unsafe { &*((l2e & ADDR_MASK) as *const PageTable) };
    let l1e = l1[l1i];
    let _ = write!(serial, "DBG ELF WALK: {} PTE=0x{:016x}\n", label, l1e);
    if l1e & PAGE_PRESENT == 0 {
        return;
    }
    let phys = (l1e & ADDR_MASK) | (vaddr & 0xfff);
    debug_dump_physical_if_identity(&mut serial, label, phys);
}

fn debug_dump_physical_if_identity(serial: &mut crate::serial::SerialPort, label: &str, phys: u64) {
    use core::fmt::Write;
    let page = phys & !0xfff;
    let identity_ok = translate(page)
        .map(|mapped| mapped & !0xfff == page)
        .unwrap_or(false);
    if !identity_ok {
        let _ = write!(
            serial,
            "DBG ELF PHYS: {} frame=0x{:016x} identity_map=unverified bytes=unavailable\n",
            label, page
        );
        return;
    }
    let _ = write!(
        serial,
        "DBG ELF PHYS: {} frame=0x{:016x} identity_map=verified bytes=",
        label, page
    );
    for i in 0..16u64 {
        let byte = unsafe { core::ptr::read_volatile((page | i) as *const u8) };
        let _ = write!(serial, "{:02x}", byte);
    }
    serial.writestrs(["\n"].as_ref());
}

pub fn debug_dump_entry_evidence(label: &str, vaddr: u64, target_pml4: u64) {
    if !cfg!(feature = "debug") {
        return;
    }
    use core::fmt::Write;
    let active = read_cr3();
    let mut serial = crate::serial::SerialPort::new();
    serial.init();
    let _ = write!(
        serial,
        "DBG ELF ENTRY: {} vaddr=0x{:016x} active_cr3=0x{:016x} target_pml4=0x{:016x}\n",
        label, vaddr, active, target_pml4
    );
    debug_dump_root_walk("active-root", vaddr, active);
    debug_dump_root_walk("target-root", vaddr, target_pml4);
    let _ = write!(serial, "DBG ELF ENTRY: {} virtual=", label);
    if translate_in_pml4(vaddr, target_pml4).is_none() {
        serial.writestrs(["unmapped\n"].as_ref());
        return;
    }
    if active != target_pml4 {
        unsafe {
            write_cr3(target_pml4);
        }
    }
    for i in 0..16u64 {
        let byte = unsafe { core::ptr::read_volatile((vaddr + i) as *const u8) };
        let _ = write!(serial, "{:02x}", byte);
    }
    if active != target_pml4 {
        unsafe {
            write_cr3(active);
        }
    }
    serial.writestrs(["\n"].as_ref());
}

pub fn debug_dump_map_site(label: &str, vaddr: u64, paddr: u64, flags: u64, target_pml4: u64) {
    if !cfg!(feature = "debug") {
        return;
    }
    use core::fmt::Write;
    let mut serial = crate::serial::SerialPort::new();
    serial.init();
    let _ = write!(serial, "DBG ELF MAP: {} vaddr=0x{:016x} paddr=0x{:016x} flags=0x{:016x} active_cr3=0x{:016x} target_pml4=0x{:016x}\n", label, vaddr, paddr, flags, read_cr3(), target_pml4);
    debug_dump_entry_evidence(label, vaddr, target_pml4);
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
    let vaddr: u64 = 0x2000_0000; // 512 MiB, outside the boot identity extension

    // Map it as 4 KiB.
    map_4k(vaddr, phys as u64, PAGE_KERNEL, pmm);
    invlpg(vaddr);

    // Write a pattern through the virtual mapping.
    unsafe {
        *(vaddr as *mut u64) = 0xDEAD_BEEF_CAFE_F00D;
    }

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

/// DEBUG-only proof that fork eagerly clones user leaf frames and cleans up a
/// partially cloned PT when the injected second-leaf allocation fails.
#[cfg(feature = "debug")]
pub fn test_fork_leaf_isolation(pmm: &mut PmmAllocator, serial: &mut crate::serial::SerialPort) {
    const FIRST: u64 = 0x0201_0000;
    const SECOND: u64 = 0x0201_1000;
    let saved_cr3 = read_cr3();
    let parent = match create_pml4(pmm, false) {
        Some(root) => root,
        None => {
            serial.writestrs(["PAGING TEST: fork leaf isolation FAIL\n"].as_ref());
            serial.writestrs(["PAGING TEST: fork leaf failure FAIL\n"].as_ref());
            return;
        }
    };
    unsafe { write_cr3(parent.phys) };
    let first_page = pmm.alloc();
    let second_page = pmm.alloc();
    let first_mapped = !first_page.is_null()
        && try_map_4k_target(FIRST, first_page as u64, PAGE_USER_RW, pmm, parent.phys);
    let second_mapped = first_mapped
        && !second_page.is_null()
        && try_map_4k_target(SECOND, second_page as u64, PAGE_USER_RW, pmm, parent.phys);
    let mapped = first_mapped && second_mapped;
    if mapped {
        unsafe {
            core::ptr::write_volatile(FIRST as *mut u8, 0x11);
            core::ptr::write_volatile(SECOND as *mut u8, 0x22);
        }
    }
    let parent_first = translate_in_pml4(FIRST, parent.phys);
    let parent_second = translate_in_pml4(SECOND, parent.phys);
    let free_before_success = pmm.available_pages();
    let child = if mapped { create_pml4(pmm, true) } else { None };
    let mut isolation_ok = mapped;
    if let Some(child_root) = child {
        let child_first = translate_in_pml4(FIRST, child_root.phys);
        let child_second = translate_in_pml4(SECOND, child_root.phys);
        isolation_ok &= child_first.is_some()
            && child_second.is_some()
            && child_first != parent_first
            && child_second != parent_second;
        unsafe { write_cr3(child_root.phys) };
        unsafe { core::ptr::write_volatile(FIRST as *mut u8, 0xCC) };
        unsafe { write_cr3(parent.phys) };
        let parent_unchanged = unsafe { core::ptr::read_volatile(FIRST as *const u8) } == 0x11;
        isolation_ok &= parent_unchanged;
        unsafe { write_cr3(saved_cr3) };
        destroy_uncommitted_pml4(child_root, pmm);
    } else {
        isolation_ok = false;
        unsafe { write_cr3(parent.phys) };
    }
    serial.writestrs(
        [
            "PAGING TEST: fork leaf isolation ",
            if isolation_ok { "PASS\n" } else { "FAIL\n" },
        ]
        .as_ref(),
    );
    let scratch_success_cleanup = pmm.available_pages() == free_before_success;
    serial.writestrs(
        [
            "PAGING TEST: scratch success cleanup ",
            if scratch_success_cleanup { "PASS\n" } else { "FAIL\n" },
        ]
        .as_ref(),
    );

    unsafe { write_cr3(parent.phys) };
    let mut expected = [0u64; 16];
    let mut expected_ok = true;
    for frame in &mut expected {
        *frame = pmm.alloc() as u64;
        if *frame == 0 {
            expected_ok = false;
            break;
        }
    }
    for frame in &expected {
        if *frame != 0 {
            pmm.free(*frame as *mut u8);
        }
    }
    let free_before_failure = pmm.available_pages();
    let injected_failure = create_pml4_with_clone_failure(pmm, true, 1).is_none();
    let scratch_failure_cleanup = pmm.available_pages() == free_before_failure;
    let parent_untouched = translate_in_pml4(FIRST, parent.phys) == parent_first
        && translate_in_pml4(SECOND, parent.phys) == parent_second;
    let mut recycled = [0u64; 16];
    for frame in &mut recycled {
        *frame = pmm.alloc() as u64;
        if *frame == 0 {
            expected_ok = false;
            break;
        }
    }
    for frame in &recycled {
        if *frame != 0 {
            pmm.free(*frame as *mut u8);
        }
    }
    expected.sort_unstable();
    recycled.sort_unstable();
    let failure_ok = injected_failure
        && parent_untouched
        && expected_ok
        && expected == recycled
        && scratch_success_cleanup
        && scratch_failure_cleanup;
    serial.writestrs(
        [
            "PAGING TEST: fork leaf failure ",
            if failure_ok { "PASS\n" } else { "FAIL\n" },
        ]
        .as_ref(),
    );

    unsafe { write_cr3(saved_cr3) };
    destroy_uncommitted_pml4(parent, pmm);
    if !first_page.is_null() && !first_mapped {
        pmm.free(first_page);
    }
    if !second_page.is_null() && !second_mapped {
        pmm.free(second_page);
    }
}
