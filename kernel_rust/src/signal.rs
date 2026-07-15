//==============================================================================
// signal.rs — Signal Handling
//
// Provides per-process signal actions, pending-signal delivery, and the
// sigreturn mechanism.  Delivery happens at three points:
//   1. syscall_handler  — before dispatching each syscall (SIG_DFL only)
//   2. scheduler_tick   — on PIT IRQ before returning to user mode
//   3. scheduler_switch  — on syscall exit before returning to user mode
//
// For paths 2+3, custom handler signals are delivered by modifying the
// iretq frame in place (pushing a signal frame on the user stack, then
// redirecting RIP/RSP/RDI/HANDLER).  For path 1, custom handlers are
// deferred to the scheduler (set should_schedule=1).
//
// SAFETY: All functions in this file are called with interrupts disabled.
// The Process and sigaction table can be safely mutated without locks.
//==============================================================================

use crate::interrupts::InterruptFrame;

//==============================================================================
// Constants
//==============================================================================

/// Default action — terminate (or resume for ignored-by-default signals).
pub const SIG_DFL: u64 = 0;
/// Ignore the signal.
pub const SIG_IGN: u64 = 1;

/// Signal numbers (only the ones we support).
pub const SIGINT: u64   = 2;
pub const SIGKILL: u64  = 9;
pub const SIGUSR1: u64  = 10;
pub const SIGUSR2: u64  = 12;

/// Number of signal slots in the sigactions array.
pub const NUM_SIGS: usize = 32;

/// Size (in bytes) of the signal frame pushed on the user stack.
pub const SIGFRAME_SIZE: u64 = 160;  // 20 qwords

//==============================================================================
// SigAction
//==============================================================================

/// Per-signal disposition.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct SigAction {
    /// 0 = SIG_DFL, 1 = SIG_IGN, >1 = function pointer to user-space handler
    pub handler: u64,
    /// Additional signals to block while this handler runs (bitmask).
    pub mask: u64,
    /// Flags: SA_NODEFER, SA_RESTART, etc.
    pub flags: u64,
}

impl Default for SigAction {
    fn default() -> Self {
        SigAction { handler: SIG_DFL, mask: 0, flags: 0 }
    }
}

//==============================================================================
// Assembly-global sigreturn buffer
//
// Written by sys_sigreturn() in Rust, read by .exit_or_block in assembly.
// The assembly reads these unconditionally when sigreturn_pending != 0.
//
// Layout of sigreturn_frame (18 qwords):
//   [0..14]  saved_rax .. saved_r15  (15 GPRs)
//   [15]     saved_rip
//   [16]     saved_rsp
//   [17]     saved_rflags
//==============================================================================

// These globals are defined in kernel/syscall_entry.asm (.data section).
// Rust uses extern declarations to access them; the assembly's `global`
// directive makes them visible to the linker.
extern "C" {
    pub static mut sigreturn_frame: [u64; 18];
    pub static mut sigreturn_pending: u8;
}

//==============================================================================
// Signal frame on user stack (20 qwords / 160 bytes)
//
// Written by deliver_pending_signals() during signal delivery, read by
// sys_sigreturn().  The frame sits below the interrupted code's user RSP.
//
// Layout:
//   [0]  saved_rax       [1]  saved_rcx        [2]  saved_rdx
//   [3]  saved_rbx       [4]  saved_rbp        [5]  saved_rsi
//   [6]  saved_rdi       [7]  saved_r8         [8]  saved_r9
//   [9]  saved_r10       [10] saved_r11        [11] saved_r12
//   [12] saved_r13       [13] saved_r14        [14] saved_r15
//   [15] saved_rflags    [16] saved_rsp        [17] saved_rip
//   [18] signum          [19] saved_mask
//==============================================================================

//==============================================================================
// Signal result type
//==============================================================================

#[derive(PartialEq)]
pub enum SignalResult {
    None,
    Terminated,
    CustomPending,  // custom handler needs scheduler to deliver
}

//==============================================================================
// Syscall-path signal check
//
// Called from syscall_handler before dispatching the syscall.
// Can only handle SIG_DFL termination and SIG_IGN; custom handlers return
// CustomPending so the caller sets should_schedule=1 and lets the scheduler
// paths (which have an iretq frame to modify) handle delivery.
//==============================================================================

pub fn check_signals_syscall(pid: u64) -> SignalResult {
    if pid == 0 || pid == 2 {
        return SignalResult::None;
    }

    let proc = crate::process::process_mut(pid);
    if proc.sig_pending == 0 {
        return SignalResult::None;
    }

    if proc.in_signal {
        // While a handler is running, SIGKILL is still fatal.
        if proc.sig_pending & (1 << SIGKILL) != 0 {
            proc.sig_pending &= !(1 << SIGKILL);
            proc.state = crate::process::ProcessState::Zombie;
            proc.exit_code = 128 + SIGKILL;
            return SignalResult::Terminated;
        }
        return SignalResult::None;
    }

    let mut result = SignalResult::None;

    for sig in 1..NUM_SIGS as u64 {
        if proc.sig_pending & (1 << sig) == 0 {
            continue;
        }

        let action = proc.sigactions[sig as usize];

        if action.handler == SIG_IGN {
            proc.sig_pending &= !(1 << sig);
            continue;
        }

        if action.handler == SIG_DFL {
            match sig {
                SIGKILL | SIGINT | SIGUSR1 | SIGUSR2 => {
                    proc.sig_pending &= !(1 << sig);
                    proc.state = crate::process::ProcessState::Zombie;
                    proc.exit_code = 128 + sig;
                    return SignalResult::Terminated;
                }
                _ => {
                    proc.sig_pending &= !(1 << sig);
                }
            }
        } else {
            // Custom handler — defer to scheduler (it has a proper frame)
            result = SignalResult::CustomPending;
        }
    }

    result
}

//==============================================================================
// Scheduler-path signal delivery
//
// Called from scheduler_tick and scheduler_switch_exit AFTER selecting the
// next process and switching to its page tables.  `krsp` points at the
// process's iretq frame (RAX slot).  Returns the (possibly modified) krsp.
//
// For custom handlers: pushes a signal frame onto the user stack, then
// modifies the kernel iretq frame to redirect to the handler.
// For SIG_DFL: sets the process Zombie and returns killed=true.
//==============================================================================

pub fn deliver_pending_signals(pid: u64, krsp: u64, killed: &mut bool) -> u64 {
    *killed = false;

    // Skip kernel-only processes
    if pid == 0 || pid == 2 {
        return krsp;
    }

    let proc = crate::process::process_mut(pid);
    if proc.sig_pending == 0 {
        return krsp;
    }

    // If a handler is already running, only SIGKILL can interrupt.
    if proc.in_signal {
        if proc.sig_pending & (1 << SIGKILL) != 0 {
            proc.sig_pending &= !(1 << SIGKILL);
            proc.state = crate::process::ProcessState::Zombie;
            proc.exit_code = 128 + SIGKILL;
            *killed = true;
        }
        return krsp;
    }

    // Only deliver to user-mode processes
    let frame = unsafe { &mut *(krsp as *mut InterruptFrame) };
    if (frame.cs & 3) != 3 {
        return krsp;
    }

    let user_rsp = frame.user_rsp;

    for sig in 1..NUM_SIGS as u64 {
        if proc.sig_pending & (1 << sig) == 0 {
            continue;
        }

        let action = proc.sigactions[sig as usize];

        if action.handler == SIG_IGN {
            proc.sig_pending &= !(1 << sig);
            continue;
        }

        if action.handler == SIG_DFL {
            match sig {
                SIGKILL | SIGINT | SIGUSR1 | SIGUSR2 => {
                    proc.sig_pending &= !(1 << sig);
                    proc.state = crate::process::ProcessState::Zombie;
                    proc.exit_code = 128 + sig;
                    *killed = true;
                    return krsp;
                }
                _ => {
                    proc.sig_pending &= !(1 << sig);
                }
            }
        } else {
            // ── Custom handler delivery ─────────────────────────────────
            proc.sig_pending &= !(1 << sig);

            // Guard: user stack must be large enough for a signal frame.
            // Uses the per-process stack_low (set by spawn_init / sys_exec).
            if user_rsp < proc.stack_low.wrapping_add(SIGFRAME_SIZE) {
                proc.state = crate::process::ProcessState::Zombie;
                proc.exit_code = 128 + SIGKILL;  // kill instead of corrupting stack
                *killed = true;
                return krsp;
            }

            let frame_rsp = user_rsp - SIGFRAME_SIZE;

            // ── Build the signal frame on the user stack ────────────────
            // Signal frame (20 qwords, 160 bytes):
            //   [0..14]  GPRs from kernel frame
            //   [15]     saved_rflags
            //   [16]     saved_rsp  (original user RSP)
            //   [17]     saved_rip  (original RIP = where signal interrupted)
            //   [18]     signal number
            //   [19]     saved_mask (proc.sig_pending at time of delivery)
            unsafe {
                // GPRs (first 15)
                let val0 = core::ptr::read((krsp +  0) as *const u64);
                let val1 = core::ptr::read((krsp +  8) as *const u64);
                let val2 = core::ptr::read((krsp + 16) as *const u64);
                let val3 = core::ptr::read((krsp + 24) as *const u64);
                let val4 = core::ptr::read((krsp + 32) as *const u64);
                let val5 = core::ptr::read((krsp + 40) as *const u64);
                let val6 = core::ptr::read((krsp + 48) as *const u64);
                let val7 = core::ptr::read((krsp + 56) as *const u64);
                let val8 = core::ptr::read((krsp + 64) as *const u64);
                let val9 = core::ptr::read((krsp + 72) as *const u64);
                let val10 = core::ptr::read((krsp + 80) as *const u64);
                let val11 = core::ptr::read((krsp + 88) as *const u64);
                let val12 = core::ptr::read((krsp + 96) as *const u64);
                let val13 = core::ptr::read((krsp + 104) as *const u64);
                let val14 = core::ptr::read((krsp + 112) as *const u64);

                let out = frame_rsp as *mut u64;
                core::ptr::write(out.add(0),  val0);
                core::ptr::write(out.add(1),  val1);
                core::ptr::write(out.add(2),  val2);
                core::ptr::write(out.add(3),  val3);
                core::ptr::write(out.add(4),  val4);
                core::ptr::write(out.add(5),  val5);
                core::ptr::write(out.add(6),  val6);
                core::ptr::write(out.add(7),  val7);
                core::ptr::write(out.add(8),  val8);
                core::ptr::write(out.add(9),  val9);
                core::ptr::write(out.add(10), val10);
                core::ptr::write(out.add(11), val11);
                core::ptr::write(out.add(12), val12);
                core::ptr::write(out.add(13), val13);
                core::ptr::write(out.add(14), val14);

                // RFLAGS, user RSP, RIP from frame struct
                core::ptr::write(out.add(15), frame.rflags);
                core::ptr::write(out.add(16), frame.user_rsp);
                core::ptr::write(out.add(17), frame.rip);

                // Signal number and saved mask
                core::ptr::write(out.add(18), sig);
                core::ptr::write(out.add(19), proc.sig_pending);
            }

            // ── Update per-process signal state ─────────────────────────
            proc.sigframe_rsp = frame_rsp;
            proc.in_signal = true;

            // ── Modify the kernel iretq frame ───────────────────────────
            // RDI = signal number (first arg to handler)
            // RIP = handler address
            // RSP = new user stack (below signal frame)
            frame.regs.rdi = sig;
            frame.rip = action.handler;
            frame.user_rsp = frame_rsp;

            return krsp;  // frame modified in-place
        }
    }

    krsp
}

//==============================================================================
// Syscall: kill(pid, sig) — send a signal to a process
//==============================================================================

pub fn sys_kill(pid: u64, sig: u64) -> i64 {
    if pid == 0 || sig == 0 || sig >= 32 {
        return -22;  // EINVAL
    }

    // Protect idle — init can receive signals (needed for tests)
    if pid == 2 {
        return 0;
    }

    let proc = crate::process::process_mut(pid);
    if proc.state == crate::process::ProcessState::Zombie {
        return -3;  // ESRCH
    }

    proc.sig_pending |= 1 << sig;

    // If the target was blocked on something (e.g., TTY read), wake it
    if proc.state == crate::process::ProcessState::Blocked {
        proc.state = crate::process::ProcessState::Ready;
    }

    0
}

//==============================================================================
// Syscall: sigaction(signum, act, oldact) — get/set signal disposition
//==============================================================================

pub fn sys_sigaction(signum: u64, act: u64, oldact: u64) -> i64 {
    if signum == 0 || signum >= 32 {
        return -22;  // EINVAL
    }
    // SIGKILL is uncatchable
    if signum == SIGKILL {
        return -22;  // EINVAL
    }

    let proc = crate::process::process_mut(crate::process::current_pid());

    // Write old action to user space
    if oldact != 0 {
        unsafe {
            core::ptr::copy_nonoverlapping(
                &proc.sigactions[signum as usize] as *const SigAction as *const u8,
                oldact as *mut u8,
                core::mem::size_of::<SigAction>(),
            );
        }
    }

    // Read new action from user space
    if act != 0 {
        unsafe {
            let mut new_action: SigAction = core::mem::zeroed();
            core::ptr::copy_nonoverlapping(
                act as *const u8,
                &mut new_action as *mut SigAction as *mut u8,
                core::mem::size_of::<SigAction>(),
            );
            proc.sigactions[signum as usize] = new_action;
        }
    }

    0
}

//==============================================================================
// Syscall: sigreturn() — return from signal handler
//
// Reads the signal frame from the user stack (at proc.sigframe_rsp), fills
// the assembly-level sigreturn_frame buffer, sets sigreturn_pending=1 and
// should_schedule=1.  The assembly .exit_or_block path will use the
// pre-built frame instead of pushing all zeros, and iretq will restore
// the original process state (all GPRs, RIP, RFLAGS, RSP).
//==============================================================================

pub fn sys_sigreturn() -> u64 {
    let proc = crate::process::process_mut(crate::process::current_pid());
    let frame_addr = proc.sigframe_rsp;
    if frame_addr == 0 {
        return 0;  // no signal context
    }

    // Read the 20-qword signal frame from user space
    let mut buf = [0u64; 20];
    unsafe {
        core::ptr::copy_nonoverlapping(frame_addr as *const u64, buf.as_mut_ptr(), 20);
    }

    // Fill the assembly sigreturn_frame
    // sigreturn_frame layout: [0..14]=RAX..R15, [15]=RIP, [16]=RSP, [17]=RFLAGS
    // Signal frame: [0..14]=GPRs, [15]=RFLAGS, [16]=RSP, [17]=RIP, [18]=signum, [19]=mask
    unsafe {
        // Copy GPRs (first 15 values)
        core::ptr::copy_nonoverlapping(buf.as_ptr(), sigreturn_frame.as_mut_ptr(), 15);
        // sigreturn_frame[15] = saved_rip = buf[17]
        sigreturn_frame[15] = buf[17];
        // sigreturn_frame[16] = saved_rsp = buf[16]
        sigreturn_frame[16] = buf[16];
        // sigreturn_frame[17] = saved_rflags = buf[15]
        sigreturn_frame[17] = buf[15];

        sigreturn_pending = 1;
    }

    // Clean up per-process state
    proc.sigframe_rsp = 0;
    proc.in_signal = false;

    // Restore the signal mask that was saved at delivery time
    // Signal frame [19] = saved_mask = the sig_pending at delivery
    proc.sig_pending = buf[19];

    // Force re-scheduling — the .exit_or_block path will use sigreturn_frame
    unsafe {
        core::ptr::write_volatile(&raw mut crate::process::should_schedule, 1);
    }

    0
}
