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

/// Syscall handler function pointer.
type SyscallFn = fn(u64, u64, u64, u64) -> u64;

/// Maximum number of syscalls supported.
const MAX_SYSCALLS: usize = 64;

/// Syscall dispatch table.
static mut SYSCALL_TABLE: [Option<SyscallFn>; MAX_SYSCALLS] = [None; MAX_SYSCALLS];

//==============================================================================
// Syscall handlers
//==============================================================================

/// Syscall 0: exit(int code) — halts the system.
///
/// In a single-process kernel, exit means the whole system halts.
fn sys_exit(code: u64, _: u64, _: u64, _: u64) -> u64 {
    let mut serial = SerialPort::new();
    let _ = core::write!(serial, "VIBIX: init exited with code {}\n", code);
    // Halt forever
    loop {
        unsafe { core::arch::asm!("hlt", options(nomem, nostack)) }
    }
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
/// Stub — not yet implemented.  Returns -1.
fn sys_read(_fd: u64, _buf: u64, _len: u64, _: u64) -> u64 {
    u64::MAX
}

/// Syscall 3: getpid() → process ID.
fn sys_getpid(_: u64, _: u64, _: u64, _: u64) -> u64 {
    1  // PID 1 for the init process
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
