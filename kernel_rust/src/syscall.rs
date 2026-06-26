//==============================================================================
// syscall.rs — SYSCALL dispatch handler
//
// Called from syscall_entry.asm after SYSCALL.
// Dispatches to registered handlers based on syscall number in RAX.
//
// Syscall ABI (VIBIX):
//   rax = syscall number
//   rdi = arg1, rsi = arg2, rdx = arg3, r8  = arg4, r9  = arg5
// Return value in rax.
//
// Phase 2 additions: fork (8), exec (9), waitpid (10).
// Phase 3: dup (18), dup2 (19).
//==============================================================================

use core::fmt::Write;
use crate::serial::SerialPort;
use crate::pmm;
use crate::process::{current_pid, process_mut, BRK_START, BRK_MAX};
use crate::process;
use crate::vfs::{MAX_FDS, EBADF, EMFILE};

/// Syscall handler function pointer.
pub type SyscallFn = fn(u64, u64, u64, u64) -> u64;

/// Maximum number of syscalls supported.
const MAX_SYSCALLS: usize = 64;

/// Syscall dispatch table.
static mut SYSCALL_TABLE: [Option<SyscallFn>; MAX_SYSCALLS] = [None; MAX_SYSCALLS];
//==============================================================================
// mmap constants (syscall 11)
//==============================================================================

/// Page protection flags
#[allow(dead_code)]
const PROT_NONE:  u64 = 0;
#[allow(dead_code)]
const PROT_READ:  u64 = 1;
#[allow(dead_code)]
const PROT_WRITE: u64 = 2;
#[allow(dead_code)]
const PROT_EXEC:  u64 = 4;

/// Mapping flags
#[allow(dead_code)]
const MAP_SHARED:    u64 = 0x01;
#[allow(dead_code)]
const MAP_PRIVATE:   u64 = 0x02;
const MAP_FIXED:     u64 = 0x10;
const MAP_ANONYMOUS: u64 = 0x20;

/// mmap failure sentinel
const MAP_FAILED: u64 = u64::MAX;


//==============================================================================
// Syscall handlers
//==============================================================================

/// Syscall 0: exit(int code) — marks process as Zombie and triggers reschedule.
fn sys_exit(code: u64, _: u64, _: u64, _: u64) -> u64 {
    let pid = current_pid();
    let mut serial = SerialPort::new();
    let _ = core::write!(serial, "VIBIX: PID {} exited with code {}\n", pid, code);

    let cur = process_mut(pid);
    cur.state = process::ProcessState::Zombie;
    cur.exit_code = code;

    // Wake parent if it's blocked waiting for us
    let parent_pid = cur.parent_pid;
    if parent_pid != 0 {
        let parent = process_mut(parent_pid);
        if parent.state == process::ProcessState::Blocked && parent.wait_for_pid == pid {
            // Patch RAX in parent's saved frame to child PID (waitpid return value)
            unsafe {
                let parent_rsp = parent.kernel_rsp;
                if parent_rsp != 0 {
                    *(parent_rsp as *mut u64) = pid;
                }
            }
            parent.state = process::ProcessState::Ready;
            parent.wait_for_pid = 0;
        }
    }

    // Signal the assembly stub to divert through scheduler
    unsafe {
        process::should_schedule = 1;
    }
    0  // value ignored; asm diverts to scheduler
}

/// Syscall 1: write(int fd, const void *buf, size_t len) → bytes written.
///
/// Dispatches through VFS: resolves fd → fd_table → OFT → vnode write op.
fn sys_write(fd: u64, buf: u64, len: u64, _: u64) -> u64 {
    if fd as usize >= MAX_FDS || buf == 0 || len == 0 {
        return 0;
    }
    let pid = current_pid();
    let proc = process_mut(pid);
    let oft_idx = proc.fd_table.fds[fd as usize];
    if oft_idx < 0 { return 0; }

    unsafe {
        let of = match crate::vfs::open_file::oft_get(oft_idx as usize) {
            Some(of) => of as *mut crate::vfs::open_file::OpenFile,
            None => return 0,
        };
        let vnode = match &mut (*of).vnode {
            Some(ref mut vn) => *vn as *mut crate::vfs::Vnode,
            None => return 0,
        };
        match (*(*vnode).ops).write {
            Some(write_fn) => {
                let nwritten = write_fn(vnode, buf as *const u8, len as usize, &mut (*of).offset);
                if nwritten < 0 { 0 } else { nwritten as u64 }
            }
            None => 0,
        }
    }
}

/// Syscall 2: read(int fd, void *buf, size_t len) → bytes read.
///
/// Dispatches through VFS: resolves fd → fd_table → OFT → vnode read op.
fn sys_read(fd: u64, buf: u64, len: u64, _: u64) -> u64 {
    if fd as usize >= MAX_FDS || buf == 0 || len == 0 {
        return u64::MAX;
    }
    let pid = current_pid();
    let proc = process_mut(pid);
    let oft_idx = proc.fd_table.fds[fd as usize];
    if oft_idx < 0 { return u64::MAX; }

    unsafe {
        let of = match crate::vfs::open_file::oft_get(oft_idx as usize) {
            Some(of) => of as *mut crate::vfs::open_file::OpenFile,
            None => return u64::MAX,
        };
        let vnode = match &mut (*of).vnode {
            Some(ref mut vn) => *vn as *mut crate::vfs::Vnode,
            None => return u64::MAX,
        };
        match (*(*vnode).ops).read {
            Some(read_fn) => {
                let nread = read_fn(vnode, buf as *mut u8, len as usize, &mut (*of).offset);
                if nread < 0 { u64::MAX } else { nread as u64 }
            }
            None => u64::MAX,
        }
    }
}

/// Syscall 3: getpid() → process ID.
fn sys_getpid(_: u64, _: u64, _: u64, _: u64) -> u64 {
    current_pid()
}

/// Syscall 5: nanosleep(sec, nsec) — busy-wait for given duration.
fn sys_nanosleep(sec: u64, nsec: u64, _: u64, _: u64) -> u64 {
    if nsec >= 1_000_000_000 {
        return u64::MAX;
    }
    let total_ticks = sec * 100 + nsec / 10_000_000;
    let start = crate::pit::get_ticks();
    loop {
        let now = crate::pit::get_ticks();
        if now.wrapping_sub(start) >= total_ticks {
            break;
        }
    }
    0
}

/// Syscall 6: uname(buf) — fill in system identification structure.
fn sys_uname(buf: u64, _: u64, _: u64, _: u64) -> u64 {
    let sysname: [u8; 6] = *b"VIBIX\0";
    let nodename: [u8; 6] = *b"vibix\0";
    let release: [u8; 6] = *b"0.1.0\0";
    let version: [u8; 27] = *b"#1 PREEMPT Tue Jun 23 2026\0";
    let machine: [u8; 7] = *b"x86_64\0";
    let domainname: [u8; 6] = *b"VIBIX\0";

    unsafe {
        core::ptr::copy_nonoverlapping(sysname.as_ptr(), buf as *mut u8, sysname.len());
        core::ptr::copy_nonoverlapping(nodename.as_ptr(), (buf + 65) as *mut u8, nodename.len());
        core::ptr::copy_nonoverlapping(release.as_ptr(), (buf + 130) as *mut u8, release.len());
        core::ptr::copy_nonoverlapping(version.as_ptr(), (buf + 195) as *mut u8, version.len());
        core::ptr::copy_nonoverlapping(machine.as_ptr(), (buf + 260) as *mut u8, machine.len());
        core::ptr::copy_nonoverlapping(domainname.as_ptr(), (buf + 325) as *mut u8, domainname.len());
    }
    0
}

#[inline]
fn outw(port: u16, val: u16) {
    unsafe { core::arch::asm!("outw %ax, %dx", in("ax") val, in("dx") port, options(att_syntax)) }
}

/// Syscall 7: reboot(magic, magic2, cmd) — reboot or power off.
fn sys_reboot(magic: u64, magic2: u64, cmd: u64, _: u64) -> u64 {
    if magic != 0xfee1dead || magic2 != 0x28121969 {
        return u64::MAX;
    }
    match cmd {
        0xcdef0123 => {
            // LINUX_REBOOT_CMD_RESTART
            outw(0x604, 0x2000);
            0
        }
        0x4321fedc => {
            // LINUX_REBOOT_CMD_POWER_OFF
            outw(0x604, 0x2000);
            0
        }
        _ => u64::MAX,
    }
}

//==============================================================================
// Phase 2 syscalls: fork, exec, waitpid
//==============================================================================

/// Syscall 8: fork() → child PID (parent) or 0 (child).
fn sys_fork(_a: u64, _b: u64, _c: u64, _d: u64) -> u64 {
    crate::process::sys_fork() as u64
}

/// Syscall 9: exec(path, argv, envp) — replace process image.
fn sys_exec(a: u64, b: u64, c: u64, _d: u64) -> u64 {
    crate::process::sys_exec(a, b, c) as u64
}

//==============================================================================
// Syscall 11: mmap — memory mapping (MAP_ANONYMOUS only)
//==============================================================================

/// Syscall 11: mmap(void *addr, size_t length, int prot, int flags, int fd, off_t offset)
///
/// For MVP, only MAP_ANONYMOUS (with or without MAP_FIXED) is supported.
/// fd and offset are ignored (must be -1 and 0 for MAP_ANONYMOUS).
fn sys_mmap(addr: u64, length: u64, _prot: u64, flags: u64) -> u64 {
    let pid = current_pid();
    let proc = process_mut(pid);

    if length == 0 {
        proc.errno = 22;   // EINVAL
        return MAP_FAILED;
    }

    let is_anon  = flags & MAP_ANONYMOUS != 0;
    let is_fixed = flags & MAP_FIXED != 0;

    if !is_anon {
        proc.errno = 38;   // ENOSYS
        return MAP_FAILED;
    }

    let page_aligned_len = (length + 0xFFF) & !0xFFF;

    let base = if is_fixed {
        if addr & 0xFFF != 0 {
            proc.errno = 22;   // EINVAL
            return MAP_FAILED;
        }
        addr
    } else {
        // Use current brk as allocation point
        let alloc = proc.brk;
        proc.brk += page_aligned_len;
        alloc
    };

    let pmm = crate::pmm::global_pmm();
    let mut vaddr = base;
    while vaddr < base + page_aligned_len {
        let phys = pmm.alloc();
        if phys.is_null() {
            proc.errno = 12;   // ENOMEM
            return MAP_FAILED;
        }
        unsafe { core::ptr::write_bytes(phys, 0, 4096); }
        crate::paging::map_4k(vaddr, phys as u64, crate::paging::PAGE_USER_RW, pmm);
        crate::paging::invlpg(vaddr);
        vaddr += 4096;
    }

    base
}


/// Syscall 10: waitpid(pid, wstatus, flags) — wait for child.
fn sys_waitpid(a: u64, b: u64, c: u64, _d: u64) -> u64 {
    crate::process::sys_waitpid(a as i64, b, c) as u64
}

//==============================================================================
// brk / sbrk — program break (heap)
//==============================================================================

/// Syscall 4: brk(void *addr) → new program break.
///
/// Convention:
///   - `addr == 0` → return current break (sbrk(0) query)
///   - `addr < BRK_START || addr > BRK_MAX` → do nothing, return -1
///   - Otherwise → set break, allocating/mapping pages as needed, return new break
fn sys_brk(addr: u64, _: u64, _: u64, _: u64) -> u64 {
    let pid = current_pid();
    let proc = process_mut(pid);

    if addr == 0 {
        return proc.brk;  // sbrk(0) query
    }

    if addr < BRK_START || addr > BRK_MAX {
        proc.errno = 38;   // ENOSYS
        return u64::MAX;
    }

    let current_page_end = (proc.brk + 0xFFF) & !0xFFF;
    let new_page_end = (addr + 0xFFF) & !0xFFF;

    if new_page_end > current_page_end {
        let pmm = pmm::global_pmm();
        let mut vaddr = current_page_end;
        while vaddr < new_page_end {
            let phys = pmm.alloc();
            if phys.is_null() {
                proc.errno = 38;
                return u64::MAX;
            }
            unsafe { core::ptr::write_bytes(phys, 0, 4096); }
            crate::paging::map_4k(vaddr, phys as u64, crate::paging::PAGE_USER_RW, pmm);
            crate::paging::invlpg(vaddr);
            vaddr += 4096;
        }
    }

    proc.brk = addr;
    addr
}

//==============================================================================
// fd-dup syscalls: dup (18), dup2 (19)
//==============================================================================

/// Syscall 18: dup(int old_fd) — duplicate a file descriptor.
///
/// Returns the new fd number on success, or -EBADF / -EMFILE on error.
fn sys_dup(old_fd: u64, _: u64, _: u64, _: u64) -> u64 {
    if old_fd as usize >= MAX_FDS {
        return (-EBADF as i64) as u64;
    }
    let pid = current_pid();
    let proc = process_mut(pid);
    let oft_idx = proc.fd_table.fds[old_fd as usize];
    if oft_idx < 0 {
        return (-EBADF as i64) as u64;
    }
    // Find first free fd
    for new_fd in 0..MAX_FDS {
        if proc.fd_table.fds[new_fd] < 0 {
            proc.fd_table.fds[new_fd] = oft_idx;
            crate::vfs::open_file::oft_incref(oft_idx as usize);
            return new_fd as u64;
        }
    }
    (-EMFILE as i64) as u64
}

/// Syscall 19: dup2(int old_fd, int new_fd) — duplicate a file descriptor to a specific number.
///
/// Follows Linux convention:
///   - If old_fd == new_fd, return new_fd (no-op).
///   - If new_fd is already open, close it first.
fn sys_dup2(old_fd: u64, new_fd: u64, _: u64, _: u64) -> u64 {
    if old_fd as usize >= MAX_FDS || new_fd as usize >= MAX_FDS {
        return (-EBADF as i64) as u64;
    }
    if old_fd == new_fd {
        return new_fd;
    }
    let pid = current_pid();
    let proc = process_mut(pid);
    let oft_idx = proc.fd_table.fds[old_fd as usize];
    if oft_idx < 0 {
        return (-EBADF as i64) as u64;
    }
    // If new_fd is already open, close it first
    let new_oft_idx = proc.fd_table.fds[new_fd as usize];
    if new_oft_idx >= 0 {
        proc.fd_table.fds[new_fd as usize] = -1;
        crate::vfs::open_file::oft_decref(new_oft_idx as usize);
    }
    // Point new_fd to the same OFT entry as old_fd
    proc.fd_table.fds[new_fd as usize] = oft_idx;
    crate::vfs::open_file::oft_incref(oft_idx as usize);
    new_fd
}

//==============================================================================
// Public API
//==============================================================================

/// Register a syscall handler.
///
/// `num` must be < 64.  Returns `true` on success.
pub fn register(num: usize, handler: SyscallFn) -> bool {
    if num >= MAX_SYSCALLS {
        return false;
    }
    unsafe {
        SYSCALL_TABLE[num] = Some(handler);
    }
    true
}

/// Initialise the syscall system.
///
/// Registers built-in handlers.  Called once during boot.
pub fn init() {
    register(0, sys_exit);
    register(1, sys_write);
    register(2, sys_read);
    register(3, sys_getpid);
    register(4, sys_brk);
    register(5, sys_nanosleep);
    register(6, sys_uname);
    register(7, sys_reboot);
    // Phase 2
    register(8, sys_fork);
    register(9, sys_exec);
    register(10, sys_waitpid);
    register(11, sys_mmap);
    // Phase 3: fd duplication
    register(18, sys_dup);
    register(19, sys_dup2);
    register(20, sys_pipe);
}

//==============================================================================
// Dispatch — called from assembly
//==============================================================================

/// Called from `syscall_entry.asm`.
///
/// # Safety
///
/// Runs in ring 0 with interrupts disabled.  Must only return via `sysretq`
/// from the assembly stub.
#[no_mangle]
pub extern "C" fn syscall_handler(
    num: u64,
    arg1: u64,
    arg2: u64,
    arg3: u64,
    arg4: u64,
) -> u64 {
    if (num as usize) < MAX_SYSCALLS {
        unsafe {
            if let Some(handler) = SYSCALL_TABLE[num as usize] {
                return handler(arg1, arg2, arg3, arg4);
            }
        }
    }
    // Unknown syscall — return -1
    u64::MAX
}

//==============================================================================
// Syscall 20: pipe(pipefd) — create a pipe
//==============================================================================

/// Syscall 20: pipe(pipefd) — create a pipe.
///
/// `pipefd` is a pointer to a user-space array of two `i32`s.
/// On success, writes `[read_fd, write_fd]` to `pipefd` and returns 0.
/// On error, returns a negative errno value.
fn sys_pipe(pipefd: u64, _: u64, _: u64, _: u64) -> u64 {
    crate::vfs::pipe::sys_pipe(pipefd) as u64
}
