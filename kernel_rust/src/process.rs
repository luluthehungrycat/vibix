//! process.rs — Multi-process scheduler
//!
//! Provides: Process, ProcessState, ProcessTable, spawn_init(),
//! start_scheduler(), scheduler_tick(), sched_next().
//! Phase 2: idle process (PID 2), fork (8), exec (9), waitpid (10).

use crate::gdt;
use crate::paging;
use crate::pmm::PmmAllocator;

const MAX_PROCS: usize = 64;
const KERNEL_STACK_SIZE: usize = 12288; // 12 KB (3 pages)
pub const USER_CODE_ADDR: u64 = 0x2000000;
const FLAT_USER_LIMIT: u64 = BRK_START;

/// BRK start address (shared constant for per-process brk)
pub const BRK_START: u64 = 0x500_0000; // Start well above the ELF and flat-binary stacks
pub const BRK_MAX: u64 = 0x1000_0000;
// SIGINT constant moved to signal.rs (crate::signal::SIGINT)
pub const WNOHANG: u64 = 1;

/// Idle process entry — runs in kernel mode, halts forever.
extern "C" fn idle_entry() -> ! {
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub enum ProcessState {
    Ready = 0,
    Running = 1,
    Blocked = 2,
    Zombie = 3,
}

#[repr(C)]
#[derive(Debug, Clone)]
pub struct Process {
    pub pid: u64,
    pub pml4_phys: u64,
    pub state: ProcessState,
    pub entry: u64,
    pub user_rsp: u64,
    pub kernel_stack_top: u64,
    pub kernel_rsp: u64,
    pub kernel_stack_base: u64,
    pub parent_pid: u64,
    pub exit_code: u64,
    pub wait_for_pid: u64,
    pub brk: u64,
    pub errno: i64,
    pub sig_pending: u64, // bitmask: bit N = signal N pending
    pub sigactions: [crate::signal::SigAction; 32],
    pub in_signal: bool,
    pub sigframe_rsp: u64,
    pub stack_low: u64,
    pub name: [u8; 32],
    pub fd_table: crate::vfs::FdTable,
    pub cwd: [u8; 256],
}

pub struct ProcessTable {
    pub slots: [Option<Process>; MAX_PROCS],
    pub next_pid: u64,
    pub count: usize,
}

// Singleton accessed only with interrupts disabled
static mut PROCESS_TABLE: ProcessTable = ProcessTable {
    slots: [const { None }; MAX_PROCS],
    next_pid: 1,
    count: 0,
};
static mut CURRENT_PID: u64 = 0;

// Assembly globals

extern "C" {
    pub static mut current_proc_kernel_rsp: u64;
}

#[repr(C)]
pub struct SyscallSavedState {
    pub rsp: u64,
    pub rflags: u64,
    pub rip: u64,
}

extern "C" {
    pub static mut syscall_state: SyscallSavedState;
}

extern "C" {
    pub static mut should_schedule: u8;
}

// --- Helpers ---

pub fn current_pid() -> u64 {
    unsafe { CURRENT_PID }
}

fn set_current_pid(pid: u64) {
    unsafe {
        CURRENT_PID = pid;
    }
}

pub fn process(pid: u64) -> &'static Process {
    unsafe {
        for slot in &PROCESS_TABLE.slots {
            if let Some(p) = slot {
                if p.pid == pid {
                    return p;
                }
            }
        }
        panic!("process {} not found", pid);
    }
}

pub fn process_mut(pid: u64) -> &'static mut Process {
    unsafe {
        for slot in &mut PROCESS_TABLE.slots {
            if let Some(p) = slot {
                if p.pid == pid {
                    return p;
                }
            }
        }
        panic!("process {} not found", pid);
    }
}

/// Return a live process for optional ownership lookups.
///
/// Unlike `process_mut`, this is safe for stale PID references held by
/// subsystems such as the TTY line discipline.
pub fn process_mut_if(pid: u64) -> Option<&'static mut Process> {
    unsafe {
        for slot in &mut PROCESS_TABLE.slots {
            if let Some(p) = slot {
                if p.pid == pid {
                    return Some(p);
                }
            }
        }
        None
    }
}

pub fn set_syscall_kstack(rsp: u64) {
    unsafe {
        current_proc_kernel_rsp = rsp;
    }
}

// --- Frame builder ---

/// Build a synthetic register frame on the kernel stack.
///
/// Frame layout (matching irq_common pop order):
///   kernel_rsp → [RAX, RCX, RDX, RBX, RBP, RSI, RDI,
///                 R8, R9, R10, R11, R12, R13, R14, R15,
///                 int_no=0, err_code=0,
///                 RIP, CS=0x23, RFLAGS=0x202, user_RSP, SS=0x1B]
///
/// Returns kernel_rsp (pointer to RAX slot).
fn build_init_frame(kernel_stack_top: u64, user_entry: u64, user_rsp: u64, command_id: u64) -> u64 {
    unsafe {
        let ptr = kernel_stack_top as *mut u64;

        // Build frame from HIGH address downward (last pushed = lowest addr).
        // iretq frame (highest addresses — popped last by iretq)
        ptr.sub(1).write(0x1Bu64); // SS
        ptr.sub(2).write(user_rsp); // user RSP
        ptr.sub(3).write(0x202u64); // RFLAGS (IF enabled)
        ptr.sub(4).write(0x23u64); // CS (user code | 3)
        ptr.sub(5).write(user_entry); // RIP

        // int_no + err_code
        ptr.sub(6).write(0u64); // err_code (dummy)
        ptr.sub(7).write(0u64); // int_no

        // GPRs — written high-to-low: R15 is at top-64 (highest),
        // RAX is at top-176 (lowest = kernel_rsp).
        // This matches the order in context_switch.asm which pops rax first.
        ptr.sub(8).write(0u64); // R15
        ptr.sub(9).write(0u64); // R14
        ptr.sub(10).write(0u64); // R13
        ptr.sub(11).write(0u64); // R12
        ptr.sub(12).write(0u64); // R11
        ptr.sub(13).write(0u64); // R10
        ptr.sub(14).write(0u64); // R9
        ptr.sub(15).write(0u64); // R8
        ptr.sub(16).write(command_id); // RDI = command selector
        ptr.sub(17).write(0u64); // RSI
        ptr.sub(18).write(0u64); // RBP
        ptr.sub(19).write(0u64); // RBX
        ptr.sub(20).write(0u64); // RDX
        ptr.sub(21).write(0u64); // RCX
        ptr.sub(22).write(0u64); // RAX

        ptr.sub(22) as u64 // = kernel_rsp (points at RAX)
    }
}

/// Build a synthetic register frame for the child of a fork().
/// Uses syscall_state saved at fork syscall entry so the child
/// returns to the instruction after the fork syscall with RAX=0.
fn build_fork_frame(kstack_top: u64) -> u64 {
    unsafe {
        let ptr = kstack_top as *mut u64;

        // iretq frame (highest addresses)
        ptr.sub(1).write(0x1Bu64); // SS
        ptr.sub(2).write(syscall_state.rsp); // user RSP
        ptr.sub(3).write(0x202u64); // RFLAGS (IF=1)
        ptr.sub(4).write(0x23u64); // CS (user code | 3)
        ptr.sub(5).write(syscall_state.rip); // RIP (after fork)

        // int_no + err_code
        ptr.sub(6).write(0u64); // err_code (dummy)
        ptr.sub(7).write(0u64); // int_no

        // GPRs — written high-to-low
        ptr.sub(8).write(0u64); // R15
        ptr.sub(9).write(0u64); // R14
        ptr.sub(10).write(0u64); // R13
        ptr.sub(11).write(0u64); // R12
        ptr.sub(12).write(0u64); // R11
        ptr.sub(13).write(0u64); // R10
        ptr.sub(14).write(0u64); // R9
        ptr.sub(15).write(0u64); // R8
        ptr.sub(16).write(0u64); // RDI
        ptr.sub(17).write(0u64); // RSI
        ptr.sub(18).write(0u64); // RBP
        ptr.sub(19).write(0u64); // RBX
        ptr.sub(20).write(0u64); // RDX
        ptr.sub(21).write(0u64); // RCX
        ptr.sub(22).write(0u64); // RAX = 0 (child gets 0)

        ptr.sub(22) as u64 // kernel_rsp = address of RAX slot
    }
}

// --- Binary loader ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlatLoadError {
    InvalidSize,
    AddressOverflow,
    Oversize,
    Oom,
    MappingCollision,
}

pub struct FlatImage {
    pub root: paging::Pml4Handle,
    pub user_rsp: u64,
    pub stack_low: u64,
}

/// Build a flat image and its post-code stack under a fresh, uncommitted root.
/// The old address space is never touched by this builder.
// Keep this transaction's stack frame bounded; DEBUG kernel_main runs on a
// fixed 16 KiB bootstrap stack.
#[inline(never)]
fn build_flat_image_inner(
    data: *const u8,
    size: usize,
    pmm: &mut PmmAllocator,
    fail_after_allocations: Option<usize>,
) -> Result<FlatImage, FlatLoadError> {
    if data.is_null() || size == 0 {
        return Err(FlatLoadError::InvalidSize);
    }
    let size_u64 = u64::try_from(size).map_err(|_| FlatLoadError::AddressOverflow)?;
    let rounded = size_u64
        .checked_add(0xfff)
        .ok_or(FlatLoadError::AddressOverflow)?;
    let pages_needed = rounded / 0x1000;
    let pages = core::cmp::max(pages_needed, 2);
    let code_bytes = pages
        .checked_mul(0x1000)
        .ok_or(FlatLoadError::AddressOverflow)?;
    let stack_addr = USER_CODE_ADDR
        .checked_add(code_bytes)
        .ok_or(FlatLoadError::AddressOverflow)?;
    let stack_end = stack_addr
        .checked_add(0x1000)
        .ok_or(FlatLoadError::AddressOverflow)?;
    if stack_end > FLAT_USER_LIMIT {
        return Err(FlatLoadError::Oversize);
    }
    if paging::overlaps_kernel_scratch(USER_CODE_ADDR, stack_end) {
        return Err(FlatLoadError::Oversize);
    }

    let root = paging::create_pml4(pmm, false).ok_or(FlatLoadError::Oom)?;
    let mut bytes_left = size;
    let mut src_offset = 0usize;
    let mut virt_addr = USER_CODE_ADDR;
    let mut allocations = 0usize;

    for _ in 0..pages {
        if fail_after_allocations == Some(allocations) {
            paging::destroy_uncommitted_pml4(root, pmm);
            return Err(FlatLoadError::Oom);
        }
        let page = pmm.alloc();
        allocations += 1;
        if page.is_null() {
            paging::destroy_uncommitted_pml4(root, pmm);
            return Err(FlatLoadError::Oom);
        }
        let copy_len = core::cmp::min(bytes_left, 0x1000);
        unsafe {
            // PMM pages may retain bytes from an earlier owner.  Clear every
            // newly allocated code page before copying, including pages with
            // no source bytes, so flat BSS/tails cannot expose stale data.
            core::ptr::write_bytes(page, 0, 0x1000);
            if copy_len != 0 {
                core::ptr::copy_nonoverlapping(data.add(src_offset), page, copy_len);
            }
        }
        if !paging::try_map_4k_target(
            virt_addr,
            page as u64,
            paging::PAGE_USER_RW,
            pmm,
            root.phys,
        ) {
            pmm.free(page);
            paging::destroy_uncommitted_pml4(root, pmm);
            return Err(FlatLoadError::MappingCollision);
        }
        paging::invlpg(virt_addr);
        src_offset = src_offset
            .checked_add(0x1000)
            .ok_or_else(|| {
                paging::destroy_uncommitted_pml4(root, pmm);
                FlatLoadError::AddressOverflow
            })?;
        virt_addr = virt_addr
            .checked_add(0x1000)
            .ok_or_else(|| {
                paging::destroy_uncommitted_pml4(root, pmm);
                FlatLoadError::AddressOverflow
            })?;
        bytes_left = bytes_left.saturating_sub(0x1000);
    }

    if fail_after_allocations == Some(allocations) {
        paging::destroy_uncommitted_pml4(root, pmm);
        return Err(FlatLoadError::Oom);
    }
    let stack_page = pmm.alloc();
    if stack_page.is_null() {
        paging::destroy_uncommitted_pml4(root, pmm);
        return Err(FlatLoadError::Oom);
    }
    if !paging::try_map_4k_target(
        stack_addr,
        stack_page as u64,
        paging::PAGE_USER_RW,
        pmm,
        root.phys,
    ) {
        pmm.free(stack_page);
        paging::destroy_uncommitted_pml4(root, pmm);
        return Err(FlatLoadError::MappingCollision);
    }
    paging::invlpg(stack_addr);
    Ok(FlatImage {
        root,
        user_rsp: stack_end,
        stack_low: stack_addr,
    })
}

pub fn build_flat_image(
    data: *const u8,
    size: usize,
    pmm: &mut PmmAllocator,
) -> Result<FlatImage, FlatLoadError> {
    build_flat_image_inner(data, size, pmm, None)
}

#[cfg(feature = "debug")]
fn build_flat_image_with_failure(
    data: *const u8,
    size: usize,
    pmm: &mut PmmAllocator,
    fail_after_allocations: usize,
) -> Result<FlatImage, FlatLoadError> {
    build_flat_image_inner(data, size, pmm, Some(fail_after_allocations))
}

#[cfg(feature = "debug")]
static FLAT_ROLLBACK_DATA: [u8; 0x1001] = [0x90; 0x1001];

#[cfg(feature = "debug")]
static FLAT_SMALL_DATA: [u8; 1] = [0xCC];

/// DEBUG-only proof that flat construction is fresh-root and failure-atomic.
#[cfg(feature = "debug")]
// Keep the fixture's large PMM-reuse arrays out of kernel_main's bootstrap
// frame while exercising the real builder.
#[inline(never)]
pub fn test_flat_image_failures(
    pmm: &mut PmmAllocator,
    serial: &mut crate::serial::SerialPort,
) -> bool {
    // Use an existing kernel-only identity mapping as the old-root sentinel.
    // The fresh builder must not alter it, and using a pre-existing branch
    // avoids allocating fixture page tables in the active root.
    const OLD_ADDR: u64 = 0x1000;
    let saved_cr3 = paging::read_cr3();
    let old_cr3 = saved_cr3;
    let old_mapping = paging::translate_in_pml4(OLD_ADDR, old_cr3);
    let old_mapped = old_mapping.is_some();
    let flat_data = FLAT_ROLLBACK_DATA.as_ptr();
    let small_data = FLAT_SMALL_DATA.as_ptr();
    // Seed several soon-to-be-reused PMM pages with non-zero data.  The fresh
    // root consumes a few table pages before its code pages, so keeping a
    // short run makes the fixture exercise reuse rather than merely observing
    // a never-before-used frame.
    let mut stale_frames = [0u64; 8];
    let mut stale_seed_ok = true;
    for frame in &mut stale_frames {
        let page = pmm.alloc();
        if page.is_null() {
            stale_seed_ok = false;
            break;
        }
        unsafe { core::ptr::write_bytes(page, 0xA5, 0x1000) };
        *frame = page as u64;
    }
    for frame in &stale_frames {
        if *frame != 0 {
            pmm.free(*frame as *mut u8);
        }
    }

    let small_binary_ok = match build_flat_image(small_data, 1, pmm) {
        Ok(image) => {
            let code_first = paging::translate_in_pml4(USER_CODE_ADDR, image.root.phys);
            let code_second = paging::translate_in_pml4(USER_CODE_ADDR + 0x1000, image.root.phys);
            let stack = paging::translate_in_pml4(image.stack_low, image.root.phys);
            let mut first_tail_zero = false;
            let mut second_page_zero = false;
            let mut stale_frame_reused = false;
            unsafe {
                if let Some(first_phys) = code_first {
                    stale_frame_reused |= stale_frames.iter().any(|frame| *frame == first_phys);
                    let first = first_phys as *const u8;
                    first_tail_zero = *first == FLAT_SMALL_DATA[0];
                    for offset in 1..0x1000 {
                        if *first.add(offset) != 0 {
                            first_tail_zero = false;
                            break;
                        }
                    }
                }
                if let Some(second_phys) = code_second {
                    stale_frame_reused |= stale_frames.iter().any(|frame| *frame == second_phys);
                    let second = second_phys as *const u8;
                    second_page_zero = true;
                    for offset in 0..0x1000 {
                        if *second.add(offset) != 0 {
                            second_page_zero = false;
                            break;
                        }
                    }
                }
            }
            let layout_ok = code_first.is_some()
                && code_second.is_some()
                && stack.is_some()
                && image.user_rsp == image.stack_low + 0x1000;
            paging::destroy_uncommitted_pml4(image.root, pmm);
            stale_seed_ok
                && stale_frame_reused
                && layout_ok
                && first_tail_zero
                && second_page_zero
        }
        Err(_) => false,
    };

    let oversize = (FLAT_USER_LIMIT - USER_CODE_ADDR) as usize;
    let oversize_ok = matches!(
        build_flat_image(flat_data, oversize, pmm),
        Err(FlatLoadError::Oversize)
    );

    let mut expected = [0u64; 32];
    let mut reuse_ok = true;
    for frame in &mut expected {
        *frame = pmm.alloc() as u64;
        if *frame == 0 {
            reuse_ok = false;
            break;
        }
    }
    for frame in &expected {
        if *frame != 0 {
            pmm.free(*frame as *mut u8);
        }
    }
    let failed = build_flat_image_with_failure(flat_data, 0x1001, pmm, 1);
    let old_state_ok = paging::read_cr3() == saved_cr3
        && paging::translate_in_pml4(OLD_ADDR, old_cr3) == old_mapping
        && old_mapped;
    let mut recycled = [0u64; 32];
    for frame in &mut recycled {
        *frame = pmm.alloc() as u64;
        if *frame == 0 {
            reuse_ok = false;
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
    reuse_ok &= expected == recycled;
    let failure_ok = matches!(failed, Err(FlatLoadError::Oom)) && old_state_ok && reuse_ok;
    serial.writestrs(
        [
            "FLAT TEST: allocation rollback ",
            if failure_ok { "PASS\n" } else { "FAIL\n" },
        ]
        .as_ref(),
    );
    serial.writestrs(
        [
            "FLAT TEST: oversize rejection ",
            if oversize_ok { "PASS\n" } else { "FAIL\n" },
        ]
        .as_ref(),
    );
    serial.writestrs(
        [
            "FLAT TEST: old-root no-overwrite ",
            if old_state_ok { "PASS\n" } else { "FAIL\n" },
        ]
        .as_ref(),
    );
    serial.writestrs(
        [
            "FLAT TEST: small-binary zero-tail/no-stale copy ",
            if small_binary_ok { "PASS\n" } else { "FAIL\n" },
        ]
        .as_ref(),
    );

    failure_ok && oversize_ok && small_binary_ok
}

// --- Init process + idle process ---

/// Create and register the init process (PID 1) and idle process (PID 2).
/// Returns PID of init.
pub fn spawn_init(pmm: &mut PmmAllocator) -> u64 {
    // Load PID 1 binary from initramfs via VFS
    let pid1_image = match crate::vfs::vfs_resolve(b"/sbin/init") {
        Ok(vn) if !vn.data.is_null() && vn.size > 0 =>
            match build_flat_image(vn.data as *const u8, vn.size as usize, pmm) {
                Ok(image) => image,
                Err(_) => loop {
                    unsafe { core::arch::asm!("hlt", options(nomem, nostack)) }
                },
            },
        _ => loop {
            unsafe { core::arch::asm!("hlt", options(nomem, nostack)) }
        },
    };
    let pid1_pml4 = pid1_image.root.phys;
    let user_rsp = pid1_image.user_rsp;
    if cfg!(feature = "debug") {
        let mut serial = crate::serial::SerialPort::new();
        serial.init();
        use core::fmt::Write;
        let _ = writeln!(serial, "DBG INIT FLAT STACK: rsp={:016x}", user_rsp);
        paging::debug_dump_walk("after spawn_init flat load", USER_CODE_ADDR, pid1_pml4);
    }
    if cfg!(feature = "debug") {
        paging::debug_dump_walk("spawn_init final", USER_CODE_ADDR, pid1_pml4);
    }

    let kstack_page = pmm.alloc_pages(3);

    if kstack_page.is_null() {
        loop {
            unsafe { core::arch::asm!("hlt", options(nomem, nostack)) }
        }
    }
    let ktop = kstack_page as u64 + KERNEL_STACK_SIZE as u64;
    // Build synthetic frame with command_id=1 (init_demo, not shell).
    // The flat loader's stack is placed after the complete binary.
    let krsp = build_init_frame(ktop, USER_CODE_ADDR, user_rsp, 1);

    let table = unsafe { &mut PROCESS_TABLE };
    table.slots[0] = Some(Process {
        pid: 1,
        pml4_phys: pid1_pml4,
        state: ProcessState::Ready,
        entry: USER_CODE_ADDR,
        user_rsp,
        kernel_stack_top: ktop,
        kernel_rsp: krsp,
        kernel_stack_base: kstack_page as u64,
        parent_pid: 0,
        exit_code: 0,
        wait_for_pid: 0,
        brk: BRK_START,
        errno: 0,
        sig_pending: 0,
        sigactions: [Default::default(); 32],
        in_signal: false,
        sigframe_rsp: 0,
        stack_low: user_rsp - 0x1000,
        name: {
            let mut n = [0u8; 32];
            let bytes = b"init\0";
            let mut i = 0;
            while i < bytes.len() {
                n[i] = bytes[i];
                i += 1;
            }
            n
        },
        fd_table: crate::vfs::FdTable::new(),
        cwd: {
            let mut c = [0u8; 256];
            c[0] = b'/';
            c
        },
    });
    table.count = 1;
    table.next_pid = 2;

    // Set up fd 0/1/2 for init — all point to /dev/ttyS0
    unsafe {
        let tty_vnode = crate::vfs::devfs::devfs_get_vnode(crate::vfs::devfs::DevId::TtyS0);
        let oft_idx = crate::vfs::open_file::oft_alloc(tty_vnode, crate::vfs::O_RDWR, 0o666);
        match oft_idx {
            Ok(idx) => {
                // All three fds share the same OpenFile entry (same offset)
                table.slots[0].as_mut().unwrap().fd_table.fds[0] = idx as i32;
                table.slots[0].as_mut().unwrap().fd_table.fds[1] = idx as i32;
                table.slots[0].as_mut().unwrap().fd_table.fds[2] = idx as i32;
                // Refcount is already 1 from oft_alloc, account for fds 1 and 2
                if let Some(ref mut of) = crate::vfs::open_file::OPEN_FILE_TABLE.entries[idx] {
                    of.refcount = 3;
                }
            }
            Err(_) => {
                // No OFT slots available — should not happen at boot
            }
        }
    }

    // ── PID 2: idle ──
    let idle_kstack = pmm.alloc_pages(3);
    if idle_kstack.is_null() {
        // No memory for idle stack — halt (should never happen)
        loop {
            unsafe {
                core::arch::asm!("hlt", options(nomem, nostack));
            }
        }
    }
    let idle_ktop = idle_kstack as u64 + KERNEL_STACK_SIZE as u64;

    // Build frame for idle — uses build_init_frame then overrides CS to kernel CS.
    // RSP must point to a valid kernel stack address (idle_ktop) because iretq
    // pops SS:RSP even for same-privilege returns in 64-bit mode. With RSP=0,
    // the first interrupt (e.g. PIT timer) would try to push onto RSP=0 and
    // cause a page fault (observed in TCG mode).
    let idle_krsp = build_init_frame(idle_ktop, idle_entry as *const () as u64, idle_ktop, 0);
    // Debug: print idle frame and kernel stack addresses (enable with `make DEBUG=1`)
    if cfg!(feature = "debug") {
        use core::fmt::Write;
        let mut serial = crate::serial::SerialPort::new();
        serial.init();
        let _ = write!(
            serial,
            "VIBIX: idle kstack_base={:016X} ktop={:016X} krsp={:016X} entry={:016X}\n",
            idle_kstack as u64, idle_ktop, idle_krsp, idle_entry as *const () as u64
        );
    }
    // Override CS and SS for kernel-mode return (CPL=0).
    // In 64-bit mode, iretq ALWAYS pops SS:RSP even for same-level returns,
    // so SS must match the target CPL (0 → kernel data segment).
    unsafe {
        let ptr = idle_krsp as *mut u64;
        // Offsets: RAX=0, ..., RIP=136, CS=144, RFLAGS=152, RSP=160, SS=168
        *ptr.add(144 / 8) = 0x08; // CS = kernel code segment (CPL=0)
        *ptr.add(168 / 8) = 0x10; // SS = kernel data segment (CPL=0)
    }

    // Create idle PML4 — kernel mappings only
    let idle_root = match crate::paging::create_pml4(pmm, false) {
        Some(root) => root,
        None => loop {
            unsafe { core::arch::asm!("hlt", options(nomem, nostack)) }
        },
    };
    table.slots[1] = Some(Process {
        pid: 2,
        pml4_phys: idle_root.phys,
        state: ProcessState::Ready,
        entry: idle_entry as *const () as u64,
        user_rsp: 0,
        kernel_stack_top: idle_ktop,
        kernel_rsp: idle_krsp,
        kernel_stack_base: idle_kstack as u64,
        parent_pid: 0,
        exit_code: 0,
        wait_for_pid: 0,
        brk: 0,
        errno: 0,
        sig_pending: 0,
        sigactions: [Default::default(); 32],
        in_signal: false,
        sigframe_rsp: 0,
        stack_low: 0,
        name: {
            let mut n = [0u8; 32];
            let bytes = b"idle\0";
            let mut i = 0;
            while i < bytes.len() {
                n[i] = bytes[i];
                i += 1;
            }
            n
        },
        fd_table: crate::vfs::FdTable::new(),
        cwd: {
            let mut c = [0u8; 256];
            c[0] = b'/';
            c
        },
    });
    table.count = 2;
    table.next_pid = 3;

    1
}

// --- Scheduler ---

fn sched_next() -> u64 {
    let cur = current_pid();
    let table = unsafe { &PROCESS_TABLE };

    let start = table
        .slots
        .iter()
        .position(|s| s.as_ref().map_or(false, |p| p.pid == cur))
        .unwrap_or(0);

    // First pass: look for any Ready process that isn't idle
    for offset in 1..MAX_PROCS {
        let idx = (start + offset) % MAX_PROCS;
        if let Some(p) = &table.slots[idx] {
            if p.state == ProcessState::Ready && p.pid != 2 {
                return p.pid;
            }
        }
    }

    // Second pass: if idle is Ready, run it
    for slot in &table.slots {
        if let Some(p) = slot {
            if p.state == ProcessState::Ready && p.pid == 2 {
                return 2;
            }
        }
    }

    cur // nothing ready (shouldn't happen with idle alive)
}

/// Called from irq_common after EOI. Interrupts disabled.
/// current_rsp points at saved RAX in the register frame.
/// Returns kernel_rsp of next process to run.
#[no_mangle]
pub extern "C" fn scheduler_tick(current_rsp: u64) -> u64 {
    // Poll input before selecting the next process. A blocked TTY reader
    // cannot poll its own serial input; tty::poll_input() delivers bytes to
    // the line discipline and wakes its waiting PID when a line completes.
    crate::vfs::tty::Tty::poll_input();

    let cur_pid = current_pid();
    if cur_pid == 0 {
        // No process yet — don't try to schedule
        return current_rsp;
    }
    let cur = process_mut(cur_pid);
    cur.kernel_rsp = current_rsp;
    cur.state = ProcessState::Ready;

    loop {
        let next_pid = sched_next();
        #[cfg(feature = "debug")]
        crate::scheduler_evidence::scheduled_out(
            cur_pid,
            next_pid,
            current_rsp,
            crate::paging::read_cr3(),
        );
        if cfg!(feature = "debug") {
            use core::fmt::Write;
            let mut serial = crate::serial::SerialPort::new();
            serial.init();
            let _ = write!(
                serial,
                "DBG sched: t={} cur={} next={}\n",
                crate::pit::get_ticks(),
                current_pid(),
                next_pid
            );
        }
        set_current_pid(next_pid);

        // Check if next process is runnable (immutable borrow)
        {
            let next = process(next_pid);
            if next.state != ProcessState::Ready {
                loop {
                    unsafe {
                        core::arch::asm!("hlt", options(nomem, nostack));
                    }
                }
            }
        }

        // Set up resources for the selected process
        unsafe {
            gdt::set_rsp0(process(next_pid).kernel_stack_top);
        }
        set_syscall_kstack(process(next_pid).kernel_stack_top);
        unsafe {
            crate::paging::write_cr3(process(next_pid).pml4_phys);
        }

        if cfg!(feature = "debug") {
            use core::fmt::Write;
            let next = process(next_pid);
            let mut serial = crate::serial::SerialPort::new();
            serial.init();
            let _ = write!(
                serial,
                "DBG sched restore: pid={} state={:?} cr3={:016x} pml4={:016x} krsp={:016x}\n",
                next_pid,
                next.state,
                crate::paging::read_cr3(),
                next.pml4_phys,
                next.kernel_rsp
            );
        }

        // Deliver pending signals (modifies iretq frame, may kill process)
        let krsp = process(next_pid).kernel_rsp;
        let mut process_killed = false;
        let final_krsp =
            crate::signal::deliver_pending_signals(next_pid, krsp, &mut process_killed);

        if process_killed {
            // Process was terminated by signal — re-select
            continue;
        }

        #[cfg(feature = "debug")]
        crate::scheduler_evidence::scheduled_in(
            next_pid,
            cur_pid,
            final_krsp,
            crate::paging::read_cr3(),
        );

        // DEBUG: inspect the frame before returning it
        if cfg!(feature = "debug") {
            use core::fmt::Write;
            let mut serial = crate::serial::SerialPort::new();
            serial.init();
            let _ = write!(
                serial,
                "DBG tick: pid={} krsp={:016x}\n",
                next_pid, final_krsp
            );
            for i in 0..22 {
                let off = i * 8;
                let val: u64 =
                    unsafe { core::ptr::read_volatile((final_krsp + off) as *const u64) };
                let _ = write!(serial, "DBG tick:  [{:3}]: {:016x}\n", off, val);
            }
        }

        let next = process_mut(next_pid);
        next.state = ProcessState::Running;
        return final_krsp;
    }
}

/// Called from syscall_entry.asm when should_schedule is set.
/// Saves the current synthetic frame and switches to next process.
#[no_mangle]
pub extern "C" fn scheduler_switch_exit(current_rsp: u64) -> u64 {
    let cur_pid = current_pid();
    let cur = process_mut(cur_pid);
    cur.kernel_rsp = current_rsp;

    if cfg!(feature = "debug") {
        use core::fmt::Write;
        let mut serial = crate::serial::SerialPort::new();
        serial.init();
        let _ = write!(
            serial,
            "DBG sw_exit: pid={} saved_krsp={:016x}\n",
            cur_pid, current_rsp
        );
    }

    // state already set by handler (e.g. Zombie for exit, Blocked for blocked I/O)
    // If the current process is still Running, mark it Ready for scheduling.
    // Explicitly-set states (Zombie, Blocked) are preserved.
    if cur.state == ProcessState::Running {
        cur.state = ProcessState::Ready;
    }

    loop {
        let next_pid = sched_next();
        #[cfg(feature = "debug")]
        crate::scheduler_evidence::scheduled_out(
            cur_pid,
            next_pid,
            current_rsp,
            crate::paging::read_cr3(),
        );
        if cfg!(feature = "debug") {
            use core::fmt::Write;
            let mut serial = crate::serial::SerialPort::new();
            serial.init();
            let _ = write!(
                serial,
                "DBG sched: t={} cur={} next={}\n",
                crate::pit::get_ticks(),
                current_pid(),
                next_pid
            );
        }
        set_current_pid(next_pid);

        // Check if next process is runnable
        {
            let next = process(next_pid);
            if next.state != ProcessState::Ready {
                loop {
                    unsafe {
                        core::arch::asm!("hlt", options(nomem, nostack));
                    }
                }
            }
        }

        // Set up resources for the selected process
        unsafe {
            gdt::set_rsp0(process(next_pid).kernel_stack_top);
        }
        set_syscall_kstack(process(next_pid).kernel_stack_top);
        unsafe {
            crate::paging::write_cr3(process(next_pid).pml4_phys);
        }

        if cfg!(feature = "debug") {
            use core::fmt::Write;
            let next = process(next_pid);
            let mut serial = crate::serial::SerialPort::new();
            serial.init();
            let _ = write!(
                serial,
                "DBG sched restore: pid={} state={:?} cr3={:016x} pml4={:016x} krsp={:016x}\n",
                next_pid,
                next.state,
                crate::paging::read_cr3(),
                next.pml4_phys,
                next.kernel_rsp
            );
        }

        // Deliver pending signals (may kill process or modify iretq frame)
        let krsp = process(next_pid).kernel_rsp;
        let mut process_killed = false;
        let final_krsp =
            crate::signal::deliver_pending_signals(next_pid, krsp, &mut process_killed);

        if process_killed {
            // Process terminated by signal — re-select
            continue;
        }

        #[cfg(feature = "debug")]
        crate::scheduler_evidence::scheduled_in(
            next_pid,
            cur_pid,
            final_krsp,
            crate::paging::read_cr3(),
        );

        let next = process_mut(next_pid);
        next.state = ProcessState::Running;
        return final_krsp;
    }
}

// --- Syscall implementations ---

/// Fork the current process. Returns child PID to parent, 0 to child.
pub fn sys_fork() -> i64 {
    let parent_pid = current_pid();
    let parent = process(parent_pid);

    // Allocate new kernel stack for child
    let child_kstack = {
        let pmm = crate::pmm::global_pmm();
        pmm.alloc_pages(3)
    };
    if child_kstack.is_null() {
        return -1; // ENOMEM
    }

    let child_base = child_kstack as u64;
    let child_ktop = child_base + KERNEL_STACK_SIZE as u64;

    // Build synthetic frame for child using saved syscall state.
    // Child returns to the instruction after fork syscall with RAX=0.

    // Create child PML4 — copy kernel + user mappings from parent
    let child_root = {
        let pmm = crate::pmm::global_pmm();
        crate::paging::create_pml4(pmm, true)
    };
    let child_root = match child_root {
        Some(root) => root,
        None => {
            crate::pmm::global_pmm().free(child_kstack);
            return -12; // ENOMEM
        }
    };
    let child_pml4 = child_root.phys;
    let child_krsp = build_fork_frame(child_ktop);

    // Assign child PID
    let child_pid = unsafe {
        let table = &mut PROCESS_TABLE;
        let pid = table.next_pid;
        table.next_pid += 1;
        pid
    };

    // Find free slot
    let free_slot = unsafe { PROCESS_TABLE.slots.iter_mut().position(|s| s.is_none()) };

    match free_slot {
        Some(idx) => {
            unsafe {
                PROCESS_TABLE.slots[idx] = Some(Process {
                    pid: child_pid,
                    pml4_phys: child_pml4,
                    state: ProcessState::Ready,
                    entry: parent.entry,
                    user_rsp: syscall_state.rsp, // use saved syscall RSP
                    kernel_stack_top: child_ktop,
                    kernel_rsp: child_krsp,
                    kernel_stack_base: child_base,
                    parent_pid: parent_pid,
                    exit_code: 0,
                    wait_for_pid: 0,
                    brk: parent.brk,
                    errno: 0,
                    sig_pending: 0,
                    sigactions: parent.sigactions,
                    in_signal: false,
                    sigframe_rsp: 0,
                    stack_low: parent.stack_low,
                    name: {
                        let mut n = [0u8; 32];
                        let bytes = b"forked\0";
                        let mut i = 0;
                        while i < bytes.len() {
                            n[i] = bytes[i];
                            i += 1;
                        }
                        n
                    },
                    fd_table: parent.fd_table,
                    cwd: parent.cwd,
                });
                // Increment refcount on shared OFT entries (child now shares them)
                {
                    let child = PROCESS_TABLE.slots[idx].as_ref().unwrap();
                    for &fd_entry in &child.fd_table.fds {
                        if fd_entry >= 0 {
                            crate::vfs::open_file::oft_incref(fd_entry as usize);
                        }
                    }
                }
                PROCESS_TABLE.count += 1;
            }
            child_pid as i64
        }
        None => {
            crate::paging::destroy_uncommitted_pml4(child_root, crate::pmm::global_pmm());
            crate::pmm::global_pmm().free(child_kstack);
            -1 // EAGAIN — no free slot
        }
    }
}

/// Exec — reload user program context from a VFS-resolved path.
/// Supports both ELF64 executables and flat binaries.
pub fn sys_exec(path: u64, _argv: u64, _envp: u64) -> i64 {
    let pid = current_pid();
    let old_proc = process(pid);
    if cfg!(feature = "debug") {
        use core::fmt::Write;
        let (saved_rip, saved_rsp) = unsafe { (syscall_state.rip, syscall_state.rsp) };
        let mut serial = crate::serial::SerialPort::new();
        serial.init();
        let _ = write!(serial, "DBG EXEC BEGIN: pid={} state={:?} cr3={:016x} pml4={:016x} rip={:016x} rsp={:016x} krsp={:016x}\n", pid, old_proc.state, crate::paging::read_cr3(), old_proc.pml4_phys, saved_rip, saved_rsp, old_proc.kernel_rsp);
    }

    // 1. Copy path string from user space
    let path_buf = unsafe {
        match crate::vfs::cstr_from_user(path as *const u8, crate::vfs::PATH_MAX) {
            Ok(buf) => buf,
            Err(e) => return -e as i64,
        }
    };
    let path_len = path_buf
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(crate::vfs::PATH_MAX);
    let path_slice = &path_buf[..path_len];

    // 2. Resolve path through VFS
    let vn = match crate::vfs::vfs_resolve(path_slice) {
        Ok(v) => v,
        Err(e) => return -e as i64,
    };

    // 3. Extract data pointer and size from vnode
    let data = vn.data as *const u8;
    let size = vn.size as usize;
    if data.is_null() || size == 0 {
        return -2; // ENOENT
    }

    if cfg!(feature = "debug") {
        use core::fmt::Write;
        let mut serial = crate::serial::SerialPort::new();
        serial.init();
        let _ = write!(serial, "DBG EXEC: path=");
        for &byte in path_slice {
            serial.writestrs(&[core::str::from_utf8(&[byte]).unwrap_or("?")]);
        }
        let magic = unsafe { core::slice::from_raw_parts(data, core::cmp::min(size, 4)) };
        let _ = write!(serial, " size={} magic=", size);
        for &byte in magic {
            let _ = write!(serial, "{:02x}", byte);
        }
        serial.writestrs(&["\n"]);
    }

    let pmm = crate::pmm::global_pmm();

    // 4. Check for ELF magic and dispatch accordingly
    let entry: u64;
    let user_rsp: u64;
    let stack_low: u64;
    let root: crate::paging::Pml4Handle;
    #[cfg(feature = "debug")]
    let mut is_elf = false;
    unsafe {
        let magic = core::slice::from_raw_parts(data, 4);
        if magic == b"ELF" {
            // ELF64 binary — build into a fresh private address space. The
            // current image is committed only after loading and stack setup.
            let data_slice = core::slice::from_raw_parts(data, size);
            match crate::elf::build_exec_image(data_slice, pmm) {
                Ok(image) => {
                    entry = image.entry;
                    user_rsp = image.user_rsp;
                    stack_low = image.stack_low;
                    root = image.root;
                    #[cfg(feature = "debug")]
                    {
                        is_elf = true;
                    }
                }
                Err(error) => {
                    return match error {
                        crate::elf::ElfError::Oom | crate::elf::ElfError::MetadataOom => -12,
                        _ => -22,
                    };
                }
            }
        } else {
            match build_flat_image(data, size, pmm) {
                Ok(image) => {
                    entry = USER_CODE_ADDR;
                    user_rsp = image.user_rsp;
                    stack_low = image.stack_low;
                    root = image.root;
                }
                Err(error) => {
                    return match error {
                        FlatLoadError::Oom => -12,
                        FlatLoadError::InvalidSize
                        | FlatLoadError::AddressOverflow
                        | FlatLoadError::Oversize
                        | FlatLoadError::MappingCollision => -22,
                    };
                }
            }
        }
    }

    // 5. Publish the fully built image only after all mappings succeeded.
    let proc = process_mut(pid);
    let saved_if: u64;
    unsafe {
        core::arch::asm!("pushfq; pop {}; cli", out(reg) saved_if, options(nostack));
        let root_phys = root.phys;
        // The old root is intentionally retained: successful replacement
        // reclamation requires a future root/leaf ownership reference-counting
        // change while forked descendants may still exist.
        proc.pml4_phys = root_phys;
        crate::paging::write_cr3(root_phys);
        proc.entry = entry;
        proc.user_rsp = user_rsp;
        core::ptr::write_volatile(&raw mut syscall_state.rip, entry);
        core::ptr::write_volatile(&raw mut syscall_state.rsp, user_rsp);
        core::ptr::write_volatile(&raw mut syscall_state.rflags, 0x202);
        if saved_if & 0x200 != 0 {
            core::arch::asm!("sti", options(nostack));
        }
    }

    #[cfg(feature = "debug")]
    if path_slice == b"/bin/vish" {
        crate::scheduler_evidence::register_target(
            pid,
            root.phys,
            unsafe { core::slice::from_raw_parts(data, size) },
            entry,
        );
    }

    #[cfg(feature = "debug")]
    if is_elf && path_slice == b"/bin/vish" {
        crate::scheduler_evidence::sysret_handoff(
            pid,
            proc.pml4_phys,
            entry,
            user_rsp,
            0x202,
            proc.kernel_rsp,
        );
    }

    proc.brk = BRK_START;
    proc.errno = 0;
    proc.sig_pending = 0; // fresh signal state for new program
    proc.in_signal = false;
    proc.sigframe_rsp = 0;
    proc.stack_low = stack_low;
    // Reset all sigactions to SIG_DFL for the new program image
    for i in 0..32 {
        proc.sigactions[i] = Default::default();
    }

    if cfg!(feature = "debug") {
        use core::fmt::Write;
        let (new_rip, new_rsp, new_rflags) =
            unsafe { (syscall_state.rip, syscall_state.rsp, syscall_state.rflags) };
        let mut serial = crate::serial::SerialPort::new();
        serial.init();
        let _ = write!(serial, "DBG EXEC STATE: pid={} state={:?} new_rip={:016x} new_rsp={:016x} flags={:016x} cr3={:016x} pml4={:016x} kernel_rsp={:016x}\n", pid, proc.state, new_rip, new_rsp, new_rflags, crate::paging::read_cr3(), proc.pml4_phys, proc.kernel_rsp);
    }

    // 6. Close all fds except 0/1/2 on exec
    for fd in 3..crate::vfs::MAX_FDS {
        let oft_idx = proc.fd_table.fds[fd];
        if oft_idx >= 0 {
            proc.fd_table.fds[fd] = -1;
            crate::vfs::open_file::oft_decref(oft_idx as usize);
        }
    }
    if cfg!(feature = "debug") {
        use core::fmt::Write;
        let (end_rip, end_rsp) = unsafe { (syscall_state.rip, syscall_state.rsp) };
        let mut serial = crate::serial::SerialPort::new();
        serial.init();
        let _ = write!(serial, "DBG EXEC END: pid={} state={:?} rip={:016x} rsp={:016x} cr3={:016x} pml4={:016x} kernel_rsp={:016x}\n", pid, proc.state, end_rip, end_rsp, crate::paging::read_cr3(), proc.pml4_phys, proc.kernel_rsp);
    }
    0
}

/// Wait for a child process. Returns PID on success, -1 on error.
/// For MVP: non-blocking — if child is Zombie, reap and return.
/// If child is still running, block the parent.
pub fn sys_waitpid(requested_pid: i64, wstatus: u64, flags: u64) -> i64 {
    let cur_pid = current_pid();

    // Find a matching child (scoped to release the immutable borrow)
    let child_info: Option<(u64, ProcessState)> = unsafe {
        let table = &PROCESS_TABLE;
        let mut info = None;
        for slot in &table.slots {
            if let Some(child) = slot {
                let match_pid = if requested_pid == -1 || child.pid == requested_pid as u64 {
                    true
                } else {
                    false
                };
                if child.parent_pid == cur_pid && match_pid {
                    info = Some((child.pid, child.state));
                    break;
                }
            }
        }
        info
    };

    match child_info {
        Some((pid, ProcessState::Zombie)) => {
            // Reap child
            if wstatus != 0 {
                unsafe {
                    *(wstatus as *mut u64) = process(pid).exit_code;
                }
            }
            pid as i64
        }
        Some((pid, _)) => {
            // Child still running
            if flags & WNOHANG != 0 {
                return 0; // Non-blocking: return 0 immediately
            }
            // Block parent until child exits
            let cur = process_mut(cur_pid);
            cur.wait_for_pid = pid;
            cur.state = ProcessState::Blocked;
            unsafe {
                should_schedule = 1;
            }
            -1
        }
        None => -1, // No matching child
    }
}

/// Start the scheduler — switch to init process. Never returns.
pub unsafe fn start_scheduler(init_pid: u64) -> ! {
    set_current_pid(init_pid);
    {
        let proc = process(init_pid);
        set_syscall_kstack(proc.kernel_stack_top);
        gdt::set_rsp0(proc.kernel_stack_top);
        // Switch to init process's page tables before jumping
        crate::paging::write_cr3(proc.pml4_phys);
        // Mark running
        process_mut(init_pid).state = ProcessState::Running;
        context_switch_to(proc.kernel_rsp)
    }
}

/// Clean up all file descriptors for a process.
/// Should be called on process exit (e.g. from sys_exit) to release all OFT references.
pub fn cleanup_fds(proc: &mut Process) {
    for fd in 0..crate::vfs::MAX_FDS {
        let oft_idx = proc.fd_table.fds[fd];
        if oft_idx >= 0 {
            proc.fd_table.fds[fd] = -1;
            crate::vfs::open_file::oft_decref(oft_idx as usize);
        }
    }
}

extern "C" {
    fn context_switch_to(kernel_rsp: u64) -> !;
}
