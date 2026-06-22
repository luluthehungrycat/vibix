//==============================================================================
// process.rs — Minimal process management and user-mode entry
//
// Provides:
//   - create_init() — load the embedded user flat binary into a process
//   - enter_user_mode() — switch to Ring 3 via iretq
//==============================================================================

use crate::paging;
use crate::pmm::PmmAllocator;

//------------------------------------------------------------------------------
// Constants
//------------------------------------------------------------------------------

/// Virtual address where the user init binary is loaded.
const USER_CODE_ADDR: u64 = 0x2000000;   // 32 MiB

/// Virtual address for the user stack page (grows downward from +0x1000).
const USER_STACK_ADDR: u64 = 0x2001000;  // 32 MiB + 4 KiB

/// Embedded flat binary — assembled by NASM from kernel/userspace_blob.asm.
/// Contains all GVIBU-ported commands with a dispatch table.  The kernel
/// selects a command by setting rdi = command_id before entering user mode.
static USER_INIT_BIN: &[u8] = include_bytes!("../../userspace_blob.bin");

//------------------------------------------------------------------------------
// Process descriptor
//------------------------------------------------------------------------------

/// Minimal process descriptor for the init process.
pub struct Process {
    #[allow(dead_code)]
    pub pid: u64,
    pub entry: u64,
    pub user_rsp: u64,
}

//------------------------------------------------------------------------------
// Process creation
//------------------------------------------------------------------------------

/// Create PID 1 (init) from the embedded flat binary.
///
/// Allocates physical pages for code and stack, copies the binary, and maps
/// both pages in the active address space with the USER flag so Ring 3 can
/// access them.
pub fn create_init(pmm: &mut PmmAllocator) -> Process {
    // ── Load user binary ──────────────────────────────────────────────────
    let code_page = pmm.alloc();
    if code_page.is_null() {
        loop { unsafe { core::arch::asm!("hlt", options(nomem, nostack)) } }
    }
    // Copy the embedded binary to the allocated physical page.
    unsafe {
        core::ptr::copy_nonoverlapping(USER_INIT_BIN.as_ptr(), code_page, USER_INIT_BIN.len());
    }
    // Map with USER + RW so Ring 3 can read/execute.
    paging::map_4k(USER_CODE_ADDR, code_page as u64, paging::PAGE_USER_RW, pmm);

    // ── Set up user stack ─────────────────────────────────────────────────
    let stack_page = pmm.alloc();
    if stack_page.is_null() {
        loop { unsafe { core::arch::asm!("hlt", options(nomem, nostack)) } }
    }
    paging::map_4k(USER_STACK_ADDR, stack_page as u64, paging::PAGE_USER_RW, pmm);

    Process {
        pid: 1,
        entry: USER_CODE_ADDR,
        user_rsp: USER_STACK_ADDR + 0x1000,  // RSP starts at top of stack page
    }
}

//------------------------------------------------------------------------------
// User-mode entry
//------------------------------------------------------------------------------

/// Switch to user mode (Ring 3) by building an iretq frame on the kernel stack.
///
/// The iretq instruction pops:
///   RIP ← [RSP], CS ← [RSP+8], RFLAGS ← [RSP+16], RSP ← [RSP+24], SS ← [RSP+32]
///
/// We push these in reverse order (SS first, RIP last) so iretq pops them
/// in the correct order.
///
/// # Safety
///
/// Must be called with interrupts disabled and a valid user address space
/// (pages mapped with USER flag).  Never returns.
pub unsafe fn enter_user_mode(proc: &Process) -> ! {
    // Ring 3 selector values from the GDT:
    //   USER_CS = 0x20 | 3 = 0x23
    //   USER_DS  = 0x18 | 3 = 0x1B
    core::arch::asm!(
        // Build iretq frame (push in reverse order).
        // We push FIRST, then set rdi, to avoid register-allocation conflicts.
        "push {uss}",       // SS  — user data segment
        "push {ursp}",      // RSP — user stack pointer
        "push {urflags}",   // RFLAGS — IF enabled (bit 9), bit 1 always set
        "push {ucs}",       // CS  — user code segment
        "push {urip}",      // RIP — entry point
        // Set rdi = command_id (0 = init_demo) for the userspace blob dispatch.
        // iretq pops the iretq frame; rdi is preserved for the user _start.
        "mov edi, 0",
        "iretq",            // Pop and jump to Ring 3
        uss = in(reg) 0x1Bu64,
        ursp = in(reg) proc.user_rsp,
        urflags = in(reg) 0x202u64,   // IF=1, reserved bit 1=1
        ucs = in(reg) 0x23u64,
        urip = in(reg) proc.entry,
        options(noreturn)
    )
}

//------------------------------------------------------------------------------
// ELF process creation
//------------------------------------------------------------------------------

/// Create a process from an ELF64 binary loaded in memory.
///
/// Allocates a stack page and returns a Process ready for enter_user_mode.
#[allow(dead_code)]
pub fn create_from_elf(elf_data: &[u8], pmm: &mut PmmAllocator) -> Option<Process> {
    // Load segments using the ELF loader
    let entry = crate::elf::load(elf_data, pmm).ok()?;

    // Allocate and map a fresh 4 KiB user stack at the standard stack address
    let stack_page = pmm.alloc();
    if stack_page.is_null() {
        return None;
    }
    paging::map_4k(USER_STACK_ADDR, stack_page as u64, paging::PAGE_USER_RW, pmm);

    Some(Process {
        pid: 1,
        entry,
        user_rsp: USER_STACK_ADDR + 0x1000,  // top of stack page
    })
}
