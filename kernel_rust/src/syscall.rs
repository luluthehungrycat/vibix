//==============================================================================
// syscall.rs — SYSCALL dispatch handler
//
// Called from syscall_entry.asm after SYSCALL.
// Dispatches to registered handlers based on syscall number in RAX.
//==============================================================================

use core::fmt::Write;
use crate::serial::SerialPort;

/// Syscall handler function pointer.
type SyscallFn = fn(u64, u64, u64, u64) -> u64;

/// Maximum number of syscalls supported.
const MAX_SYSCALLS: usize = 64;

/// Syscall dispatch table.
static mut SYSCALL_TABLE: [Option<SyscallFn>; MAX_SYSCALLS] = [None; MAX_SYSCALLS];

//------------------------------------------------------------------------------
// Built-in syscalls (debug/test)
//------------------------------------------------------------------------------

/// Syscall 0: test handler — prints arguments and returns 0.
fn sys_test(a: u64, b: u64, c: u64, d: u64) -> u64 {
    let mut serial = SerialPort::new();
    serial.init();
    serial.writestrs(&["VIBIX: syscall 0 (test): args="]);
    // Use write! for formatted output
    let _ = core::write!(serial, "0x{a:X} 0x{b:X} 0x{c:X} 0x{d:X}\n");
    0
}

/// Syscall 1: write to serial (debug output).
fn sys_write(ch: u64, _b: u64, _c: u64, _d: u64) -> u64 {
    let mut serial = SerialPort::new();
    serial.init();
    serial.putchar(ch as u8 as char);
    1
}

//------------------------------------------------------------------------------
// Public API
//------------------------------------------------------------------------------

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
    register(0, sys_test);
    register(1, sys_write);
}

//------------------------------------------------------------------------------
// Dispatch — called from assembly
//------------------------------------------------------------------------------

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
