//! process.rs — Multi-process scheduler
//!
//! Provides: Process, ProcessState, ProcessTable, spawn_init(),
//! start_scheduler(), scheduler_tick(), sched_next().
//! Phase 2: idle process (PID 2), fork (8), exec (9), waitpid (10).

use crate::paging;
use crate::pmm::PmmAllocator;
use crate::gdt;

const MAX_PROCS: usize = 64;
const KERNEL_STACK_SIZE: usize = 12288;  // 12 KB (3 pages)
const USER_CODE_ADDR: u64 = 0x2000000;
const USER_STACK_ADDR: u64 = 0x2002000;


/// BRK start address (shared constant for per-process brk)
pub const BRK_START: u64 = 0x201_0000;
pub const BRK_MAX: u64 = 0x1000_0000;
pub const SIGINT: u64 = 2;  // signal number 2 = SIGINT

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
    pub sig_pending: u64,    // bitmask: bit N = signal N pending
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

/// Build a synthetic register frame for the child of a fork().
/// Uses syscall_state saved at fork syscall entry so the child
/// returns to the instruction after the fork syscall with RAX=0.
fn build_fork_frame(kstack_top: u64) -> u64 {
    unsafe {
        let ptr = kstack_top as *mut u64;

        // iretq frame (highest addresses)
        ptr.sub(1).write(0x1Bu64);                        // SS
        ptr.sub(2).write(syscall_state.rsp);               // user RSP
        ptr.sub(3).write(0x202u64);                        // RFLAGS (IF=1)
        ptr.sub(4).write(0x23u64);                         // CS (user code | 3)
        ptr.sub(5).write(syscall_state.rip);               // RIP (after fork)

        // int_no + err_code
        ptr.sub(6).write(0u64);                            // err_code (dummy)
        ptr.sub(7).write(0u64);                            // int_no

        // GPRs — written high-to-low
        ptr.sub(8).write(0u64);                            // R15
        ptr.sub(9).write(0u64);                            // R14
        ptr.sub(10).write(0u64);                           // R13
        ptr.sub(11).write(0u64);                           // R12
        ptr.sub(12).write(0u64);                           // R11
        ptr.sub(13).write(0u64);                           // R10
        ptr.sub(14).write(0u64);                           // R9
        ptr.sub(15).write(0u64);                           // R8
        ptr.sub(16).write(0u64);                           // RDI
        ptr.sub(17).write(0u64);                           // RSI
        ptr.sub(18).write(0u64);                           // RBP
        ptr.sub(19).write(0u64);                           // RBX
        ptr.sub(20).write(0u64);                           // RDX
        ptr.sub(21).write(0u64);                           // RCX
        ptr.sub(22).write(0u64);                           // RAX = 0 (child gets 0)

        ptr.sub(22) as u64  // kernel_rsp = address of RAX slot
    }
}

// --- Binary loader ---


/// Load a flat binary from a raw data pointer to user pages.
/// Maps pages at USER_CODE_ADDR (code) and USER_STACK_ADDR (stack).
pub fn load_flat_binary(data: *const u8, size: usize, pmm: &mut PmmAllocator) {
    use core::cmp::min;
    let mut bytes_left = size;
    let mut src_offset = 0usize;
    let mut virt_addr = USER_CODE_ADDR;

    // Allocate enough pages for the binary (minimum 2 like before)
    let min_pages = 2;
    let pages_needed = (size + 0xfff) / 0x1000;
    let pages = if pages_needed < min_pages { min_pages } else { pages_needed };

    for _ in 0..pages {
        let page = pmm.alloc();
        if page.is_null() {
            loop { unsafe { core::arch::asm!("hlt", options(nomem, nostack)) } }
        }
        let copy_len = min(bytes_left, 0x1000);
        unsafe {
            core::ptr::copy_nonoverlapping(
                data.add(src_offset),
                page,
                copy_len,
            );
        }
        paging::map_4k(virt_addr, page as u64, paging::PAGE_USER_RW, pmm);
        paging::invlpg(virt_addr);  // Flush TLB for this page
        src_offset += 0x1000;
        virt_addr += 0x1000;
        bytes_left = bytes_left.saturating_sub(0x1000);
    }

    // Allocate and map stack page
    let stack_page = pmm.alloc();
    if stack_page.is_null() {
        loop { unsafe { core::arch::asm!("hlt", options(nomem, nostack)) } }
    }
    paging::map_4k(USER_STACK_ADDR, stack_page as u64, paging::PAGE_USER_RW, pmm);
    paging::invlpg(USER_STACK_ADDR);  // Flush TLB for stack page
}

// --- Init process + idle process ---

/// Create and register the init process (PID 1) and idle process (PID 2).
/// Returns PID of init.
pub fn spawn_init(pmm: &mut PmmAllocator) -> u64 {
    // Load PID 1 binary from initramfs via VFS
    if let Ok(vn) = crate::vfs::vfs_resolve(b"/sbin/init") {
        let data = vn.data as *const u8;
        let size = vn.size as usize;
        if !data.is_null() && size > 0 {
            load_flat_binary(data, size, pmm);
        }
    }

    // ── PID 1: init ──
    let kstack_page = pmm.alloc_pages(3);

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
        sig_pending: 0,
        name: {
            let mut n = [0u8; 32];
            let bytes = b"init\0";
            let mut i = 0;
            while i < bytes.len() { n[i] = bytes[i]; i += 1; }
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
        let oft_idx = crate::vfs::open_file::oft_alloc(
            tty_vnode,
            crate::vfs::O_RDWR,
            0o666,
        );
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
        loop { unsafe { core::arch::asm!("hlt", options(nomem, nostack)); } }
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
        let _ = write!(serial, "VIBIX: idle kstack_base={:016X} ktop={:016X} krsp={:016X} entry={:016X}\n",
            idle_kstack as u64, idle_ktop, idle_krsp, idle_entry as *const () as u64);
    }
    // Override CS and SS for kernel-mode return (CPL=0).
    // In 64-bit mode, iretq ALWAYS pops SS:RSP even for same-level returns,
    // so SS must match the target CPL (0 → kernel data segment).
    unsafe {
        let ptr = idle_krsp as *mut u64;
        // Offsets: RAX=0, ..., RIP=136, CS=144, RFLAGS=152, RSP=160, SS=168
        *ptr.add(144/8) = 0x08;  // CS = kernel code segment (CPL=0)
        *ptr.add(168/8) = 0x10;  // SS = kernel data segment (CPL=0)
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
        sig_pending: 0,
        name: {
            let mut n = [0u8; 32];
            let bytes = b"idle\0";
            let mut i = 0;
            while i < bytes.len() { n[i] = bytes[i]; i += 1; }
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
        pmm.alloc_pages(3)
    };
    if child_kstack.is_null() {
        return -1; // ENOMEM
    }

    let child_base = child_kstack as u64;
    let child_ktop = child_base + KERNEL_STACK_SIZE as u64;

    // Build synthetic frame for child using saved syscall state.
    // Child returns to the instruction after fork syscall with RAX=0.
    let child_krsp = build_fork_frame(child_ktop);

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
                    user_rsp: syscall_state.rsp,  // use saved syscall RSP
                    kernel_stack_top: child_ktop,
                    kernel_rsp: child_krsp,
                    kernel_stack_base: child_base,
                    parent_pid: parent_pid,
                    exit_code: 0,
                    wait_for_pid: 0,
                    brk: parent.brk,
                    errno: 0,
                    sig_pending: 0,
                    name: {
                        let mut n = [0u8; 32];
                        let bytes = b"forked\0";
                        let mut i = 0;
                        while i < bytes.len() { n[i] = bytes[i]; i += 1; }
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
        None => -1, // EAGAIN — no free slot
    }
}

/// Exec — reload user program context from a VFS-resolved path.
/// Supports both ELF64 executables and flat binaries.
pub fn sys_exec(path: u64, _argv: u64, _envp: u64) -> i64 {
    let pid = current_pid();
    let proc = process_mut(pid);

    // 1. Copy path string from user space
    let path_buf = unsafe {
        match crate::vfs::cstr_from_user(path as *const u8, crate::vfs::PATH_MAX) {
            Ok(buf) => buf,
            Err(e) => return -e as i64,
        }
    };
    let path_len = path_buf.iter().position(|&b| b == 0).unwrap_or(crate::vfs::PATH_MAX);
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

    let pmm = crate::pmm::global_pmm();

    // 4. Check for ELF magic and dispatch accordingly
    let entry: u64;
    let mut is_elf = false;
    unsafe {
        let magic = core::slice::from_raw_parts(data, 4);
        if magic == b"ELF" {
            // ELF64 binary — use the ELF loader
            let data_slice = core::slice::from_raw_parts(data, size);
            match crate::elf::load(data_slice, pmm) {
                Ok(ep) => {
                    // ELF loader maps segments but NOT the user stack.
                    // Place stack at a high address (0x2005000) to avoid
                    // overlapping ELF segments that extend past 0x2002000.
                    let stack_page = pmm.alloc();
                    if stack_page.is_null() {
                        return -12; // ENOMEM
                    }
                    let elf_stack: u64 = 0x2005000;
                    crate::paging::map_4k(
                        elf_stack,
                        stack_page as u64,
                        crate::paging::PAGE_USER_RW,
                        pmm,
                    );
                    paging::invlpg(elf_stack);
                    // Map a second page for larger stack depth
                    let stack_page2 = pmm.alloc();
                    if !stack_page2.is_null() {
                        crate::paging::map_4k(
                            elf_stack - 0x1000,
                            stack_page2 as u64,
                            crate::paging::PAGE_USER_RW,
                            pmm,
                        );
                        paging::invlpg(elf_stack - 0x1000);
                    }
                    entry = ep;
                    is_elf = true;
                }
                Err(e) => {
                    return match e {
                        crate::elf::ElfError::BadMagic
                        | crate::elf::ElfError::BadClass
                        | crate::elf::ElfError::BadEndian
                        | crate::elf::ElfError::BadMachine
                        | crate::elf::ElfError::BadType
                        | crate::elf::ElfError::Truncated => -22,  // EINVAL
                        crate::elf::ElfError::Oom => -12,           // ENOMEM
                    };
                }
            }
        } else {
            // Flat binary — use load_flat_binary (handles stack page internally)
            load_flat_binary(data, size, pmm);
            entry = USER_CODE_ADDR;
            is_elf = false;
        }
    }

    // 5. Update syscall_state with new entry point and stack
    // ELF binaries use 0x2005000 to avoid conflicting with ELF segments;
    // flat binaries use the traditional USER_STACK_ADDR + 0x1000.
    unsafe {
        let user_rsp = if is_elf { 0x2006000u64 } else { USER_STACK_ADDR + 0x1000 };
        core::ptr::write_volatile(&raw mut syscall_state.rip, entry);
        core::ptr::write_volatile(&raw mut syscall_state.rsp, user_rsp);
        core::ptr::write_volatile(&raw mut syscall_state.rflags, 0x202);
    }

    proc.brk = BRK_START;
    proc.errno = 0;
    proc.sig_pending = 0;  // child gets fresh signal state

    // 6. Close all fds except 0/1/2 on exec
    for fd in 3..crate::vfs::MAX_FDS {
        let oft_idx = proc.fd_table.fds[fd];
        if oft_idx >= 0 {
            proc.fd_table.fds[fd] = -1;
            crate::vfs::open_file::oft_decref(oft_idx as usize);
        }
    }
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
