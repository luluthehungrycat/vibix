//! process.rs — Multi-process scheduler
//!
//! Provides: Process, ProcessState, ProcessTable, spawn_init(),
//! start_scheduler(), scheduler_tick(), sched_next().
//! Phase 1: single init process (PID 1) only.

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

// --- Init process ---

/// Create and register the init process (PID 1). Returns PID.
pub fn spawn_init(pmm: &mut PmmAllocator) -> u64 {
    load_init_binary(pmm);

    // Allocate kernel stack for init
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
    1
}

// --- Scheduler ---

fn sched_next() -> u64 {
    let cur = current_pid();
    let table = unsafe { &PROCESS_TABLE };

    let start = table.slots.iter().position(|s| {
        s.as_ref().map_or(false, |p| p.pid == cur)
    }).unwrap_or(0);

    for offset in 1..MAX_PROCS {
        let idx = (start + offset) % MAX_PROCS;
        if let Some(p) = &table.slots[idx] {
            if p.state == ProcessState::Ready {
                return p.pid;
            }
        }
    }
    cur  // no other ready process — stay on current
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
