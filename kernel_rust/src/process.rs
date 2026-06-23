//! process.rs — Multi-process scheduler
//!
//! Provides: Process, ProcessState, ProcessTable, spawn_init(),
//! start_scheduler(), scheduler_tick(), sched_next().
//! Phase 2: idle process (PID 2), fork (8), exec (9), waitpid (10).

use crate::paging;
use crate::pmm::PmmAllocator;
use crate::gdt;

const MAX_PROCS: usize = 64;
const KERNEL_STACK_SIZE: usize = 4096;
const USER_CODE_ADDR: u64 = 0x2000000;
const USER_CODE_PAGES: u64 = 2;
const USER_STACK_ADDR: u64 = 0x2002000;

static USER_INIT_BIN: &[u8] = include_bytes!("../../userspace/vibix_blob.bin");

/// BRK start address (shared constant for per-process brk)
pub const BRK_START: u64 = 0x201_0000;
pub const BRK_MAX: u64 = 0x1000_0000;

/// Idle process entry — runs in kernel mode, halts forever.
extern "C" fn idle_entry() -> ! {
    loop {
        unsafe { core::arch::asm!("hlt", options(nomem, nostack)); }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub enum ProcessState {
    Ready   = 0,
    Running = 1,
    Blocked = 2,
    Zombie  = 3,
}

#[repr(C)]
#[derive(Debug, Clone)]
pub struct Process {
    pub pid: u64,
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
    pub name: [u8; 32],
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
    unsafe { CURRENT_PID = pid; }
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

pub fn set_syscall_kstack(rsp: u64) {
    unsafe { current_proc_kernel_rsp = rsp; }
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
fn build_init_frame(
    kernel_stack_top: u64,
    user_entry: u64,
    user_rsp: u64,
    command_id: u64,
) -> u64 {
    unsafe {
        let ptr = kernel_stack_top as *mut u64;

        // Build frame from HIGH address downward (last pushed = lowest addr).
        // iretq frame (highest addresses — popped last by iretq)
        ptr.sub(1).write(0x1Bu64);                 // SS
        ptr.sub(2).write(user_rsp);                // user RSP
        ptr.sub(3).write(0x202u64);                // RFLAGS (IF enabled)
        ptr.sub(4).write(0x23u64);                 // CS (user code | 3)
        ptr.sub(5).write(user_entry);              // RIP

        // int_no + err_code
        ptr.sub(6).write(0u64);                    // err_code (dummy)
        ptr.sub(7).write(0u64);                    // int_no

        // GPRs — written high-to-low: R15 is at top-64 (highest),
        // RAX is at top-176 (lowest = kernel_rsp).
        // This matches the order in context_switch.asm which pops rax first.
        ptr.sub(8).write(0u64);                    // R15
        ptr.sub(9).write(0u64);                    // R14
        ptr.sub(10).write(0u64);                   // R13
        ptr.sub(11).write(0u64);                   // R12
        ptr.sub(12).write(0u64);                   // R11
        ptr.sub(13).write(0u64);                   // R10
        ptr.sub(14).write(0u64);                   // R9
        ptr.sub(15).write(0u64);                   // R8
        ptr.sub(16).write(command_id);             // RDI = command selector
        ptr.sub(17).write(0u64);                   // RSI
        ptr.sub(18).write(0u64);                   // RBP
        ptr.sub(19).write(0u64);                   // RBX
        ptr.sub(20).write(0u64);                   // RDX
        ptr.sub(21).write(0u64);                   // RCX
        ptr.sub(22).write(0u64);                   // RAX

        ptr.sub(22) as u64   // = kernel_rsp (points at RAX)
    }
}

// --- Binary loader ---

fn load_init_binary(pmm: &mut PmmAllocator) {
    use core::cmp::min;
    let mut bytes_left = USER_INIT_BIN.len();
    let mut src_offset = 0usize;
    let mut virt_addr = USER_CODE_ADDR;

    for _ in 0..USER_CODE_PAGES {
        let page = pmm.alloc();
        if page.is_null() {
            loop { unsafe { core::arch::asm!("hlt", options(nomem, nostack)) } }
        }
        let copy_len = min(bytes_left, 0x1000);
        unsafe {
            core::ptr::copy_nonoverlapping(
                USER_INIT_BIN.as_ptr().add(src_offset),
                page,
                copy_len,
            );
        }
        paging::map_4k(virt_addr, page as u64, paging::PAGE_USER_RW, pmm);
        src_offset += 0x1000;
        virt_addr += 0x1000;
        bytes_left = bytes_left.saturating_sub(0x1000);
    }

    let stack_page = pmm.alloc();
    if stack_page.is_null() {
        loop { unsafe { core::arch::asm!("hlt", options(nomem, nostack)) } }
    }
    paging::map_4k(USER_STACK_ADDR, stack_page as u64, paging::PAGE_USER_RW, pmm);
}

// --- Init process + idle process ---

/// Create and register the init process (PID 1) and idle process (PID 2).
/// Returns PID of init.
pub fn spawn_init(pmm: &mut PmmAllocator) -> u64 {
    load_init_binary(pmm);

    // ── PID 1: init ──
    let kstack_page = pmm.alloc();
    if kstack_page.is_null() {
        loop { unsafe { core::arch::asm!("hlt", options(nomem, nostack)) } }
    }
    let ktop = kstack_page as u64 + KERNEL_STACK_SIZE as u64;

    // Build synthetic frame with command_id=1 (init_demo, not shell)
    let krsp = build_init_frame(ktop, USER_CODE_ADDR, USER_STACK_ADDR + 0x1000, 1);

    let table = unsafe { &mut PROCESS_TABLE };
    table.slots[0] = Some(Process {
        pid: 1,
        state: ProcessState::Ready,
        entry: USER_CODE_ADDR,
        user_rsp: USER_STACK_ADDR + 0x1000,
        kernel_stack_top: ktop,
        kernel_rsp: krsp,
        kernel_stack_base: kstack_page as u64,
        parent_pid: 0,
        exit_code: 0,
        wait_for_pid: 0,
        brk: BRK_START,
        errno: 0,
        name: {
            let mut n = [0u8; 32];
            let bytes = b"init\0";
            let mut i = 0;
            while i < bytes.len() { n[i] = bytes[i]; i += 1; }
            n
        },
    });
    table.count = 1;
    table.next_pid = 2;

    // ── PID 2: idle ──
    let idle_kstack = pmm.alloc();
    if idle_kstack.is_null() {
        // No memory for idle stack — halt (should never happen)
        loop { unsafe { core::arch::asm!("hlt", options(nomem, nostack)); } }
    }
    let idle_ktop = idle_kstack as u64 + KERNEL_STACK_SIZE as u64;

    // Build frame for idle — uses build_init_frame then overrides CS to kernel CS
    let idle_krsp = build_init_frame(idle_ktop, idle_entry as *const () as u64, 0, 0);
    // Override CS to kernel code segment since idle runs in kernel mode
    unsafe {
        // kernel_rsp points at RAX. Offsets: RAX=0, RCX=8, ..., RIP=136, CS=144
        let ptr = idle_krsp as *mut u64;
        *ptr.add(144/8) = 0x08;  // CS = kernel code segment (CPL=0)
    }

    table.slots[1] = Some(Process {
        pid: 2,
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
        name: {
            let mut n = [0u8; 32];
            let bytes = b"idle\0";
            let mut i = 0;
            while i < bytes.len() { n[i] = bytes[i]; i += 1; }
            n
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

    let start = table.slots.iter().position(|s| {
        s.as_ref().map_or(false, |p| p.pid == cur)
    }).unwrap_or(0);

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

    cur  // nothing ready (shouldn't happen with idle alive)
}

/// Called from irq_common after EOI. Interrupts disabled.
/// current_rsp points at saved RAX in the register frame.
/// Returns kernel_rsp of next process to run.
#[no_mangle]
pub extern "C" fn scheduler_tick(current_rsp: u64) -> u64 {
    let cur_pid = current_pid();
    if cur_pid == 0 {
        // No process yet — don't try to schedule
        return current_rsp;
    }
    let cur = process_mut(cur_pid);
    cur.kernel_rsp = current_rsp;
    cur.state = ProcessState::Ready;

    let next_pid = sched_next();
    set_current_pid(next_pid);

    let next = process(next_pid);
    if next.state != ProcessState::Ready {
        loop { unsafe { core::arch::asm!("hlt", options(nomem, nostack)); } }
    }

    let next = process_mut(next_pid);
    unsafe { gdt::set_rsp0(next.kernel_stack_top); }
    set_syscall_kstack(next.kernel_stack_top);

    next.state = ProcessState::Running;
    next.kernel_rsp
}

/// Called from syscall_entry.asm when should_schedule is set.
/// Saves the current synthetic frame and switches to next process.
#[no_mangle]
pub extern "C" fn scheduler_switch_exit(current_rsp: u64) -> u64 {
    let cur_pid = current_pid();
    let cur = process_mut(cur_pid);
    cur.kernel_rsp = current_rsp;
    // state already set by handler (Zombie or Blocked)

    let next_pid = sched_next();
    set_current_pid(next_pid);

    // Check if next process is actually runnable
    let next = process(next_pid);
    if next.state != ProcessState::Ready {
        // No runnable process — halt CPU indefinitely
        loop { unsafe { core::arch::asm!("hlt", options(nomem, nostack)); } }
    }

    let next = process_mut(next_pid);
    unsafe { gdt::set_rsp0(next.kernel_stack_top); }
    set_syscall_kstack(next.kernel_stack_top);

    next.state = ProcessState::Running;
    next.kernel_rsp
}

// --- Syscall implementations ---

/// Fork the current process. Returns child PID to parent, 0 to child.
pub fn sys_fork() -> i64 {
    let parent_pid = current_pid();
    let parent = process(parent_pid);

    // Allocate new kernel stack for child
    let child_kstack = {
        let pmm = crate::pmm::global_pmm();
        pmm.alloc()
    };
    if child_kstack.is_null() {
        return -1; // ENOMEM
    }

    // Copy parent's kernel stack contents to child
    let child_base = child_kstack as u64;
    unsafe {
        core::ptr::copy_nonoverlapping(
            parent.kernel_stack_base as *const u8,
            child_base as *mut u8,
            KERNEL_STACK_SIZE,
        );
    }

    // Calculate child's kernel_rsp (same offset from base as parent)
    let krsp_offset = parent.kernel_rsp - parent.kernel_stack_base;
    let child_krsp = child_base + krsp_offset;

    // Set child's RAX to 0 (offset 0 from kernel_rsp)
    unsafe { *(child_krsp as *mut u64) = 0; }

    // Assign child PID
    let child_pid = unsafe {
        let table = &mut PROCESS_TABLE;
        let pid = table.next_pid;
        table.next_pid += 1;
        pid
    };

    // Find free slot
    let free_slot = unsafe {
        PROCESS_TABLE.slots.iter_mut().position(|s| s.is_none())
    };

    match free_slot {
        Some(idx) => {
            unsafe {
                PROCESS_TABLE.slots[idx] = Some(Process {
                    pid: child_pid,
                    state: ProcessState::Ready,
                    entry: parent.entry,
                    user_rsp: parent.user_rsp,
                    kernel_stack_top: child_base + KERNEL_STACK_SIZE as u64,
                    kernel_rsp: child_krsp,
                    kernel_stack_base: child_base,
                    parent_pid: parent_pid,
                    exit_code: 0,
                    wait_for_pid: 0,
                    brk: parent.brk,
                    errno: 0,
                    name: {
                        let mut n = [0u8; 32];
                        let bytes = b"forked\0";
                        let mut i = 0;
                        while i < bytes.len() { n[i] = bytes[i]; i += 1; }
                        n
                    },
                });
                PROCESS_TABLE.count += 1;
            }
            child_pid as i64
        }
        None => -1, // EAGAIN — no free slot
    }
}

/// Exec — reload user program context.
/// For MVP, updates syscall_state so the next sysretq jumps to the binary entry.
pub fn sys_exec(_path: u64, _argv: u64, _envp: u64) -> i64 {
    let pid = current_pid();
    let proc = process_mut(pid);

    // Update syscall_state with new entry point and stack.
    // When syscall_handler returns, syscall_entry.asm loads these into
    // RCX/R11/RSP and executes sysretq, jumping to the new entry.
    unsafe {
        syscall_state.rip = USER_CODE_ADDR;
        syscall_state.rsp = USER_STACK_ADDR + 0x1000;
        syscall_state.rflags = 0x202;
    }

    proc.brk = BRK_START;
    proc.errno = 0;
    0
}

/// Wait for a child process. Returns PID on success, -1 on error.
/// For MVP: non-blocking — if child is Zombie, reap and return.
/// If child is still running, block the parent.
pub fn sys_waitpid(requested_pid: i64, wstatus: u64, _flags: u64) -> i64 {
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
                unsafe { *(wstatus as *mut u64) = process(pid).exit_code; }
            }
            pid as i64
        }
        Some((pid, _)) => {
            // Child still running — block parent
            let cur = process_mut(cur_pid);
            cur.wait_for_pid = pid;
            cur.state = ProcessState::Blocked;
            unsafe { should_schedule = 1; }
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
        // Mark running
        process_mut(init_pid).state = ProcessState::Running;
        context_switch_to(proc.kernel_rsp)
    }
}

extern "C" {
    fn context_switch_to(kernel_rsp: u64) -> !;
}
