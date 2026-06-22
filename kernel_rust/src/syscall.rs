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
use crate::paging;
use crate::pmm;

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
/// fd=0 (stdin) reads from the serial port (COM1).
/// Other fds return -1 (unsupported).
fn sys_read(fd: u64, buf: u64, len: u64, _: u64) -> u64 {
    if fd != 0 {
        return u64::MAX;  // unsupported fd
    }
    if buf == 0 || len == 0 {
        return 0;
    }
    let slice = unsafe { core::slice::from_raw_parts_mut(buf as *mut u8, len as usize) };
    let serial = SerialPort::new();
    serial.read(slice) as u64
}

/// Syscall 3: getpid() → process ID.
fn sys_getpid(_: u64, _: u64, _: u64, _: u64) -> u64 {
    1  // PID 1 for the init process
}

//==============================================================================
// errno — per-process error number (currently single-process global)
//==============================================================================

/// Kernel-internal errno value.
static mut ERRNO: i64 = 0;

pub fn set_errno(e: i64) {
    unsafe { ERRNO = e; }
}

#[allow(dead_code)]
pub fn get_errno() -> i64 {
    unsafe { ERRNO }
}

const ENOSYS: i64 = 38;

//==============================================================================
// brk / sbrk — program break (heap)
//==============================================================================

/// Base of the user heap area.
const BRK_START: u64 = 0x201_0000;   // 32 MiB + 64 KiB

/// Maximum program break (256 MiB — generous for the current process model).
const BRK_MAX: u64   = 0x1000_0000;  // 256 MiB

/// Current program break (initialised to BRK_START).
static mut PROGRAM_BREAK: u64 = BRK_START;

/// Syscall 4: brk(void *addr) → new program break.
///
/// Convention:
///   - `addr == 0` → return current break (sbrk(0) query)
///   - `addr < BRK_START || addr > BRK_MAX` → do nothing, return -1
///   - Otherwise → set break, allocating/mapping pages as needed, return new break
fn sys_brk(addr: u64, _: u64, _: u64, _: u64) -> u64 {
    unsafe {
        // sbrk(0) query — return current break
        if addr == 0 {
            return PROGRAM_BREAK;
        }

        // Validate bounds
        if addr < BRK_START || addr > BRK_MAX {
            set_errno(ENOSYS);  // closest match: not enough space
            return u64::MAX;
        }

        let current_page_end = (PROGRAM_BREAK + 0xFFF) & !0xFFF;
        let new_page_end = (addr + 0xFFF) & !0xFFF;

        if new_page_end > current_page_end {
            // Expand — allocate and map new pages
            let pmm = pmm::global_pmm();
            let mut vaddr = current_page_end;
            while vaddr < new_page_end {
                let phys = pmm.alloc();
                if phys.is_null() {
                    set_errno(ENOSYS);  // ENOMEM equivalent
                    return u64::MAX;
                }
                // Zero the page before mapping (security: no kernel-data leaks)
                core::ptr::write_bytes(phys, 0, 4096);
                paging::map_4k(vaddr, phys as u64, paging::PAGE_USER_RW, pmm);
                paging::invlpg(vaddr);
                vaddr += 4096;
            }
        }
        // Note: shrinking (new_page_end < current_page_end) could unmap pages,
        // but we keep them for simplicity.  The break address is still updated
        // so subsequent allocations don't reuse the "shrunk" space incorrectly.

        PROGRAM_BREAK = addr;
        addr
    }
}

//==============================================================================
// Public API
//==============================================================================

/// Return the current errno value.
#[allow(dead_code)]
pub fn errno_value() -> i64 {
    get_errno()
}

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
