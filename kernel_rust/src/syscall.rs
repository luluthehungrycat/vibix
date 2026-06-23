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
//==============================================================================

use core::fmt::Write;
use crate::serial::SerialPort;
use crate::pmm;
use crate::process::{current_pid, process_mut, BRK_START, BRK_MAX};
use crate::process;

/// Syscall handler function pointer.
type SyscallFn = fn(u64, u64, u64, u64) -> u64;

/// Maximum number of syscalls supported.
const MAX_SYSCALLS: usize = 64;

/// Syscall dispatch table.
static mut SYSCALL_TABLE: [Option<SyscallFn>; MAX_SYSCALLS] = [None; MAX_SYSCALLS];

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

    // Signal the assembly stub to divert through scheduler
    unsafe {
        process::should_schedule = 1;
    }
    0  // value ignored; asm diverts to scheduler
}

/// Syscall 1: write(int fd, const char *buf, size_t len) → bytes written.
///
/// Currently only fd=1 (stdout, mapped to serial) is supported.
fn sys_write(fd: u64, buf: u64, len: u64, _: u64) -> u64 {
    if fd != 1 {
        return 0;  // unsupported fd
    }
    let mut serial = SerialPort::new();
    let slice = unsafe { core::slice::from_raw_parts(buf as *const u8, len as usize) };
    for &byte in slice {
        serial.putchar(byte as char);
    }
    len
}

/// Syscall 2: read(int fd, void *buf, size_t len) → bytes read.
///
/// fd=0 (stdin) reads from:
///   1. PS/2 keyboard ring buffer (IRQ1 — used with QEMU graphical display)
///   2. Fall back to serial port COM1 (used with `-serial stdio`)
/// Other fds return -1 (unsupported).
fn sys_read(fd: u64, buf: u64, len: u64, _: u64) -> u64 {
    if fd != 0 {
        return u64::MAX;  // unsupported fd
    }
    if buf == 0 || len == 0 {
        return 0;
    }
    let slice = unsafe { core::slice::from_raw_parts_mut(buf as *mut u8, len as usize) };

    // Try PS/2 keyboard ring buffer first.
    let mut count = crate::keyboard::read(slice);

    // Fall back to serial port (COM1) if keyboard had no data.
    if count == 0 {
        let serial = SerialPort::new();
        count = serial.read(slice);
    }

    count as u64
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
