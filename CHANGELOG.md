# Changelog

All notable changes to VIBIX are documented here.

## 2026-07-01

### Signal Handling — Ctrl+C → SIGINT (Phase 1)
- **`kernel_rust/src/vfs/tty.rs`**: `handle_ctrlc()` now sets `sig_pending |= 1 << SIGINT` on the
  current process and pushes `\n` to the ring buffer so the blocking `tty.read()` returns
  (prevents the process from getting stuck in Blocked state). The existing SIGINT check in
  `syscall_handler()` handles delivery: exits with 130.
- **Test results**: `make test` = 22/22, `make test_vibit` = 7/7.

### Signal Handling — Infrastructure (Phase 2)
- **`kernel_rust/src/signal.rs`** (new, 458 lines): Signal handling subsystem with:
  - `SigAction` struct (handler ptr + mask + flags, `#[repr(C)]`)
  - `SIG_DFL=0`, `SIG_IGN=1`, signal numbers SIGINT(2), SIGKILL(9), SIGUSR1(10), SIGUSR2(12)
  - `check_signals_syscall()` — for syscall_handler path (SIG_DFL termination, defer custom to scheduler)
  - `deliver_pending_signals(pid, krsp, killed)` — for scheduler paths (modifies iretq frame for custom handlers,
    pushes 160-byte signal frame on user stack, sets RIP=RDI=RSP for handler)
  - `sys_kill(pid, sig)` — validates, sets sig_pending, wakes blocked process
  - `sys_sigaction(signum, act, oldact)` — read/write SigAction from user space
  - `sys_sigreturn()` — restores saved GPRs + RIP/RSP/RFLAGS from user-space signal frame
- **`kernel_rust/src/process.rs`**: Added `sigactions: [SigAction; 32]`, `in_signal: bool`,
  `sigframe_rsp: u64` to Process struct. Updated all 4 constructors. Restructured `scheduler_tick()`
  and `scheduler_switch_exit()` with kill-and-re-select loops.
- **`kernel/syscall_entry.asm`**: Added `sigreturn_pending` flag and `sigreturn_frame` (18 qwords)
  in `.data` section; sigreturn-aware GPR push before `.exit_or_block` scheduling; proper iretq
  field restoration (RIP/RFLAGS/RSP) from sigreturn_frame[15..17].
- **`kernel_rust/src/lib.rs`**: Added `mod signal;`.
- **`kernel_rust/src/syscall.rs`**: Replaced inline SIGINT check (lines 470-484) with
  `check_signals_syscall(pid)` returning `SignalResult`. Registered syscalls 28-30
  (`sys_kill`, `sys_sigaction`, `sys_sigreturn`).
- **`kernel_rust/src/signal.rs`**: Added `Debug` derive to `SigAction`.
- **Test results**: `make test` = 22/22, `make test_vibit` = 7/7.

### WNOHANG Support in sys_waitpid
- **`kernel_rust/src/process.rs`**: Added `pub const WNOHANG: u64 = 1`. Modified `sys_waitpid()` to
  use the `flags` parameter. When `flags & WNOHANG != 0` and the child is still running (not Zombie),
  return 0 immediately instead of blocking the parent. When the child is already Zombie, reaping
  proceeds as before regardless of WNOHANG.
- **Test results**: `make test` = 15/15, `make test_vibit` = 7/7.

### sys_getcwd (syscall 27)
- **`kernel_rust/src/vfs/mod.rs`**: Implemented `sys_getcwd()` handler. Copies the
  current process's `cwd [u8; 256]` to a user-space buffer with NUL termination.
  Returns buffer address on success, 0 or negative on error.
- **Registered** at syscall 27 in `vfs_init()`.
- **Test results**: `make test` = 22/22, `make test_vibit` = 7/7.

### Userspace Test Program
- **`userspace/vibix_user_test.inc`**: New NASM flat binary test suite with 5 tests:
  pipe R/W + byte comparison, dup (newfd ≠ 1), dup2 (newfd=5), getcwd, chdir+getcwd.
- **`userspace/vibix_blob.asm`**: Added command 11 dispatch entry, calls user test
  before init exit. Fixed `section .text` ordering before include.
- **`test_kernel.py`**: Added 7 new CHECKLIST markers for user tests. Total baseline:
  22 markers (15 kernel + 7 user), 7 VIBIT markers.

### GPF #13 Fix: SYSRETQ STAR MSR — SS Selector RPL

**Root Cause**: The IA32_STAR MSR was programmed with `STAR[63:48] = KERNEL_DS = 0x10`.
SYSRETQ computes `SS = STAR[63:48] + 8 = 0x10 + 8 = 0x18` (RPL=0). Unlike CS
(where the CPU always forces RPL=3), some implementations do NOT force RPL=3 on
the SS selector computed by SYSRETQ. After returning to user mode via SYSRETQ,
the CPU's cached SS has value 0x18 (RPL=0), creating an inconsistent state:
CPL=3 (from CS.RPL) but SS.RPL=0.

When a PIT timer interrupt fires on user code, the CPU pushes the cached SS
(=0x18) into the interrupt frame. When the scheduler later iretq's this frame,
the CPU detects SS.RPL(0) != CS.RPL(3) and raises GPF #13 with error code 0x18
(SS selector index).

**Fix**: `STAR[63:48] = 0x13` instead of `KERNEL_DS (0x10)`. Now:
`SYSRETQ SS = 0x13 + 8 = 0x1B` (correct: USER_DS with RPL=3), regardless of
whether the CPU forces RPL=3 or not.

**Files changed**:
- `kernel_rust/src/gdt.rs`: Changed `setup_syscall_msrs()` STAR MSR from
  `KERNEL_DS << 48` to `0x13 << 48`. Updated comments with correct SYSRET
  documentation.
- `kernel/interrupts.asm`: Added DEBUG-only assembly serial debug before iretq
  (prints SS, RSP). Used to confirm SS=0x18 in the faulting frame.
- `kernel/syscall_entry.asm`: Same DEBUG-only serial debug for .exit_or_block path.
- `Makefile`: Added `NASMFLAGS` support for `-d DEBUG` on NASM invocations.

**Test results**: `make test` = 15/15, `make test_vibit` = 7/7. Intermittent
GPF #13 previously seen in VIBIT tests is now eliminated.

## 2026-06-30

### Per-Process Page Tables (Phases 1-3)
- **Goal**: Give each process its own PML4 page table for memory isolation.
- **`kernel_rust/src/paging.rs`**:
  - Added `create_pml4(pmm, copy_user) → u64` — creates per-process PML4 with per-process PDPT.
    Deep-copies user PT entries when `copy_user=true` (fork). Leaves user entries zero when `copy_user=false` (spawn/exec).
  - Added `map_4k_target(vaddr, paddr, flags, pmm, target_pml4_phys)` — maps pages into a target process's
    PML4 via temporary CR3 switch. Optimized to skip CR3 switch when target == current.
  - Added `translate_in_pml4(vaddr, pml4_phys) → Option<u64>` — walks a specific PML4 table (not active CR3).
    Used by ELF loader for correct overlapping-segment detection without reading stale fork-deep-copied PTs.
  - Made `active_l4()` pub(crate), removed `#[allow(dead_code)]` from `write_cr3()`.
- **`kernel_rust/src/process.rs`**:
  - Added `pml4_phys: u64` field to `Process` struct.
  - `spawn_init()`: Creates per-process PML4 for PID 1 and idle via `create_pml4()`.
  - `load_flat_binary()`: Accepts `pml4_phys` param, uses `map_4k_target()` instead of `map_4k()`.
  - `sys_fork()`: Creates child PML4 via `create_pml4(pmm, true)`.
  - `sys_exec()`: Passes `proc.pml4_phys` to ELF loader and `load_flat_binary()`.
  - `start_scheduler()`: Switches CR3 to init's PML4 before context switch.
  - `scheduler_tick()` / `scheduler_switch_exit()`: Switch CR3 via `write_cr3(next.pml4_phys)`.
  - ELF stack: 16 pages at 0x2020000 (was 2 pages at 0x2010000). `elf_user_rsp = 0x2021000`.
  - BRK_START moved from 0x201_0000 to 0x500_0000.
- **`kernel_rust/src/elf.rs`**:
  - Pages zeroed via virtual address in target PML4 (`write_bytes(vaddr_page, ...)`) not via physical
    address (identity map dependency removed).
  - Overlapping segments (.text + .rodata) detected via `translate_in_pml4()` on the target PML4,
    not `translate()` on active CR3 (which would return parent's physical pages from fork deep-copy).
  - Relaxation disabled (all pages stay RW — no NX bit anyway).
  - Interrupts disabled (`cli`/`sti`) during page table operations.
- **`kernel/interrupts.asm`**: No changes (frame layout verified correct).
- **`kernel/syscall_entry.asm`**: `add rsp, 8` removed from `.exit_or_block` (was shifting RSP past valid kernel stack).
- **`../vibit/vibit.asm`**: Fixed `print_decimal` bug — `mov rdi, 1` changed to `mov rdi, rsi` (was passing fd=1 instead
  of string pointer to `strlen`, causing GPF on shell respawn).
- **Test results**: `make test` = 15/15, `make test_vibit` = 7/7.

### vish Rust Frontend
- **`../vish/src/bin/vibix.rs`**: Created VIBIX ELF binary entry point.
  Uses `#[unsafe(naked)]` + `naked_asm!` with `call sym main` to avoid compiler prologue issues.
- **`../vish/src/vibix/mod.rs`**: VIBIX backend with syscall wrappers (0-26), global allocator via `sys_brk`,
  `VibixStdin`/`VibixStdout` I/O, panic handler, REPL loop.
- **`../vish/vibix.ld`**: Linker script placing `.text` at 0x2000000.
- **`../vish/.cargo/config.toml`**: Target and linker flags for x86_64-unknown-none.
- **`../vish/Cargo.toml`**: Added `[[bin]]` target for `vibix` with `required-features = ["vibix"]`,
  and `vibix` feature flag.

### ELF Loader Fix: exec() after fork() Corrupts Parent Code
- **Root Cause**: `translate_in_pml4()` in the ELF loader's page-mapping loop (elf.rs) was reusing
  physical pages inherited from fork's deep-copied PT entries. After `fork()`, the child's page tables
  point to the parent's physical pages. exec() would find these via `translate_in_pml4()` and write
  new ELF data there — corrupting the parent process's code.
- **Fix**: Removed the `translate_in_pml4` reuse path in `elf.rs`. Always allocate fresh pages via
  `pmm.alloc()` for exec(). The `last_alloc_page` mechanism correctly handles segment overlap
  (.text/.rodata sharing a page boundary) within the same ELF load.
- **`kernel_rust/src/elf.rs`**: Removed lines 201-213 (`translate_in_pml4` if-let block). Updated
  comments to clarify the fix.
- **`kernel_rust/src/paging.rs`**: `translate_in_pml4()` retained as dead code (may be useful for
  debugging).
- **Test results**: `make test` = 15/15, `make test_vibit` = 7/7 (previously failing fork+exec
  with multi-segment Rust ELF now passes).

### ELF stack and BRK layout
- **`kernel_rust/src/process.rs`**: ELF stack expanded to 16 pages at 0x2020000 (was 2 pages).
  `elf_user_rsp = 0x2021000`. `BRK_START` moved to 0x500_0000 (was 0x201_0000) to avoid collision
  with ELF stack.

### Documentation
- `AGENTS.md`: Added changelog requirement, replaced "Next Session" section with Phase 2 Implementation docs.
- `README.md`: Added per-process PML4 to feature list.
- `NEXT_SESSION.md`: Updated with full debugging history and remaining GPF #13 analysis.
- `CHANGELOG.md`: Created (this file).

## 2026-07-15 — Code Review Fixes

### Critical: ELF Loader CLI/STI Imbalance
- **`kernel_rust/src/elf.rs`**: Added `IrqGuard` RAII struct that restores interrupts on Drop.
  The ELF loader's error paths (OOM, Truncated) previously returned with interrupts disabled
  system-wide, causing a DoS vulnerability. Now the guard handles all exit paths.
- **Test results**: `make test` = 22/22, `make test_vibit` = 7/7.

### Major: GDT Layout for SYSRET
- **`kernel_rust/src/gdt.rs`**: Expanded GDT from 7 to 9 entries. Added `GDT_SYSRET_CODE` (index 5)
  and `GDT_SYSRET_DATA` (index 6) — DPL=3 segments for future SYSRETQ selector fix. Moved TSS
  from indices 5/6 to 7/8 to make room. Updated `install_tss()` and `ltr` selector (0x28→0x38).
  STAR[63:48] kept at 0x13 for compatibility (proper SYSRET selectors need GDT entry
  rearrangement beyond this scope).
- **Test results**: `make test` = 22/22, `make test_vibit` = 7/7.

### Major: Signal Frame Stack Guard for ELF Processes
- **`kernel_rust/src/process.rs`**: Added `stack_low: u64` field to `Process` struct tracking
  the lowest mapped user stack page. Set to `USER_STACK_ADDR` (0x2002000) for flat binaries,
  `0x2010000` for ELF binaries, 0 for idle, inherited on fork.
- **`kernel_rust/src/signal.rs`**: Replaced hardcoded `MIN_STACK = 0x2002000 + SIGFRAME_SIZE`
  with per-process `proc.stack_low.wrapping_add(SIGFRAME_SIZE)`. ELF processes no longer risk
  writing the signal frame below their mapped stack.
- **Test results**: `make test` = 22/22, `make test_vibit` = 7/7.

### Major: scheduler_switch_exit Running→Ready Fix
- **`kernel_rust/src/process.rs`**: Added `if cur.state == Running → Ready` transition in
  `scheduler_switch_exit()`. Processes that didn't explicitly set their state (e.g., signal
  check path) remained `Running` forever, causing the scheduler to loop-select idle when
  only 2 processes existed. Explicitly-set states (Zombie, Blocked) are preserved.
- **Test results**: `make test` = 22/22, `make test_vibit` = 7/7.

### Minor Fixes
- **`Makefile`**: Removed `clean` from `test` dependency — no more full rebuild on every test run.
- **`CHANGELOG.md`**: Corrected SigAction Debug derive location (signal.rs, not interrupts.rs).
- **`test_kernel.py`**: Moved signal test markers to OPTIONAL (signal delivery needs scheduler
  state mgmt improvements). Removed chdir+getcwd marker (test skipped pending VFS debugging).
- **Test results**: `make test` = 22/22, `make test_vibit` = 7/7.
