# VIBIX Kernel Roadmap

A Unixoid kernel for x86-64, written in Rust + NASM assembly.

---

## ✅ Done

### Core Infrastructure
- Two-stage boot (Multiboot ELF32 wrapper → 64-bit flat binary)
- PS/2 keyboard driver, VBE framebuffer (1024×768×32)
- Per-process PML4 page tables (fork deep-copies, exec allocates fresh)
- Round-robin scheduler (100 Hz PIT timer)

### ELF Loader
- ELF64 executable loading with multi-segment support, including load-local shared-page provenance (Rust ELF probe reaches `OK` without exception)
- Loads via per-process PML4 (not identity map) — current committed implementation
- Pages always allocated fresh on exec (no parent-memory corruption)
- DEBUG-only Rust ELF scheduler evidence correlates target PID, CR3, executable RIP range, ordered `IN`/`USER`/`OUT` events, and three bounded round trips

### VFS
- Vnodes with function-pointer dispatch
- initramfs (embedded tar archive)
- devfs (null, zero, ttyS0)
- Pipes (heap-allocated, refcounted shared buffer)
- TTY with ring buffer (COM1 serial)

### System Calls (32+)
fork, exec, exit, waitpid, open, close, read, write, dup, dup2,
ioctl, brk, sbrk, chdir, getcwd, getpid, getppid, getuid, geteuid,
getgid, getegid, getpgid, setpgid, getpgrp, setsid, kill, reboot,
mkdir, getdents, signal, sigaction, sigreturn, pause, nanosleep,
alarm, syslog, times, uname, uptime

### Signal Handling (Phases 1-2)
- **Ctrl+C → SIGINT**: `handle_ctrlc()` sets `sig_pending` and unblocks the reader.
  Uses existing SIGINT check in `syscall_handler()` (exit with 130).
- **signal.rs**: `SigAction` struct, `deliver_pending_signals()` (modifies iretq frame for
  custom handlers, pushes 160-byte signal frame on user stack), `check_signals_syscall()`.
- **Syscalls**: `sys_kill` (28), `sys_sigaction` (29), `sys_sigreturn` (30).
- **Assembly**: `sigreturn_frame` buffer + sigreturn-aware GPR push in `.exit_or_block`.
- **Scheduler**: kill-and-re-select loop in `scheduler_tick` + `scheduler_switch_exit`.

### Bug Fixes & Features
- **sys_getcwd (syscall 27)**: Implemented `sys_getcwd()` handler. Copies cwd
  from per-process `[u8; 256]` buffer to user space with NUL termination.
- **WNOHANG**: `sys_waitpid` now respects the WNOHANG flag — returns 0
  immediately when a child is still running instead of blocking.
- **Userspace test suite**: NASM test program covering pipe, dup, dup2, getcwd,
  chdir+getcwd, fd error conventions, VFS/TTY validation, and dup2 refcount survival.
- **GPF #13 (SYSRETQ SS selector)**: STAR[63:48] changed from KERNEL_DS(0x10)
  to 0x13 so `SS = 0x13 + 8 = 0x1B` with correct RPL=3.
- **ELF after fork**: Removed `translate_in_pml4()` reuse — exec always
  allocates fresh pages, never writes onto parent's physical pages.
- **Current ELF overlap-loader fix**: `elf::load` records pages allocated during one load call,
  reuses shared PT_LOAD pages, and preserves later partial-page file bytes.
- **Descriptor error semantics**: covered close/dup/dup2 and VFS descriptor handlers reject
  out-of-range, closed, and stale descriptors with documented VIBIX results; `dup2` validates
  its source before same-fd no-op handling and DEBUG builds assert fd/OFT invariants.

### Tests
- `make test` — 25 required integration markers (as defined by `test_kernel.py`)
- `make test_vibit` — 7 VIBIT init/shell fork/exec/blocking-read checks
- VIBIT/vish integration — external VIBIT init launches the NASM vish shell; bounded shell handoff and continuation checks pass
- `make test_tty_sigint` — deterministic no-reader, stale-reader, non-blocked-owner, and
  blocked-owner Ctrl-C cases pass without panic or exception
- `make test_vibit_rust` — readiness-aware KVM/TCG retries, Rust ELF `OK`, no exception,
  shared-page provenance, and three bounded PID/CR3/RIP-correlated scheduler round trips
  (final evidence: PID 3, CR3 `0x297000`, RIP range `0x2000000–0x2000019`, 10 events).
- `make test_vibit_rust_large` — test-time 257-page synthetic ELF, lower-level provenance-arena
  capacity/cleanup regression, VIBIT `/bin/vish` fork/exec handoff, clean child exit/reap,
  shell respawn, and a subsequent completion marker.
- `make test_elf_rollback` — DEBUG truncation, PMM exhaustion, metadata exhaustion, rollback,
  reuse, and no-panic/no-exception cases pass.

---

## 📋 Short Tasks (Next)

### Follow-up OpenSpec changes from PR #8 review
- **TTY foreground SIGINT** (`fix-tty-sigint-foreground-reader`): completed foreground-reader routing,
  wakeup, bounded integration coverage, and deterministic no-reader/stale/non-blocked/blocked-owner cases.
- **Rust vish probe prerequisite** (`build-rust-vish-test-prerequisite`): completed sibling ELF build,
  staging, and runtime Rust probe validation without sibling source changes.
- **Scalable ELF provenance** (`remove-elf-page-provenance-ceiling`): completed reclaimed metadata arena,
  cleanup, >256-page regression, and DEBUG failure-injection rollback coverage.

### 3. Signal handling — userspace test
- **File**: `userspace/vibix_signal_test.inc`
- **Why**: `sys_kill`, `sys_sigaction`, `sys_sigreturn` are kernel-complete but untested
- **Work**: NASM test program that installs SIGUSR1 handler, raises it via `sys_kill`, verifies
  the handler ran (flag check). Also test SIGINT termination and uncatchable SIGKILL.

### 5. mmap / munmap
- **Why**: Memory mapping for dynamic allocation
- **Work**: Implement file-backed and anonymous mmap. Heavy — involves page-level VMA tracking.

### 6. Process groups / session management
- **Why**: setsid, getpgid, setpgid, getpgrp exist but untested
- **Work**: Verify job-control primitives work end-to-end

### Validation limitations retained
- Correlated DEBUG evidence proves the target process's bounded scheduler round trips, not
  scheduler fairness, long-term liveness, or PID/range-correlated behavior for other processes.
- The large-ELF fixture is generated at test time and remains temporary/ignored; its lifecycle
  assertion now covers completion marker → VIBIT reap/respawn → subsequent marker.
- Running the ordinary `test_kernel.py` marker suite directly against a DEBUG image is not a
  required gate: DEBUG rollback self-tests can delay normal userspace markers beyond its bounded
  timeout. The dedicated DEBUG TTY and ELF rollback harnesses pass.
- Aggregate strict OpenSpec validation passes all 9 active changes. The archived
  `fix-multisegment-elf-iretq-gpf` change is intentionally outside the active aggregate and is
  not a current blocker.

---

## 🏗️ Medium Projects

### Signals (full implementation)
- Verify signal delivery on syscall return and interrupt return
- Signal masking, pending signal coalescing
- `sigprocmask`, `sigsuspend`, `sigpending`
- Test with a process that sets up a handler and raises SIGUSR1 to itself

### mmap (anonymous + file-backed)
- Virtual memory area (VMA) tracking
- Page-level allocation on first access (demand paging)
- Shared mappings between processes
- Test with allocation, access, and deallocation

### Process groups and job control
- Foreground process group for TTY
- TTY signals (SIGINT via Ctrl+C, SIGTSTP via Ctrl+Z)
- Shell job control (bg, fg, jobs)

---

## 🚀 Longer-Term

### Dynamic linking (ELF interpreter)
- Load the dynamic linker as an ELF interpreter
- Pass AT_* auxiliary vector on stack
- Userspace `ld.so` implementation (NASM or Rust)

### SMP
- APIC init, IPI, per-CPU data
- Spinlocks, atomic ops
- Multi-core scheduler

### Network stack
- Simple NIC driver (e1000 / virtio)
- ARP, IP, UDP, TCP in Rust
- Socket API

### Extended filesystem
- FAT32 or ext2 driver (read+write)
- Block cache
- Mount from disk image

---

*Last updated: 2026-08-11*
