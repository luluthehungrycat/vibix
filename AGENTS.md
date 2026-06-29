# AGENTS.md
*Guidelines for coding agents (OpenCode) working with VIBIX.*

---

## **General Rules**
1. **Primary workflow**: OpenCode agents handle all development — Rust kernel code, NASM assembly, build system, and testing.
2. **Orchestration**: High-level planning, architecture, and multi-phase refactors use the orchestrator with deepwork skill. Implementation is delegated to specialist agents (fixer, coder).
3. **Avoid**:
   - Copying foreign kernel source code verbatim (anti-plagiarism checker scans for Linux/BSD patterns).
   - Re-using any GPL-licensed source code.
   - Using Mistral models (known flaky behavior with low-level kernel code).

---

## **Agent Workflow**

### New feature / syscall
1. **Orchestrator** plans interface (arguments, return type, error handling)
2. **Fixer/Coder** implements Rust handler in `kernel_rust/src/syscall.rs`
3. **Orchestrator** verifies via `make run` in QEMU

### New assembly routine
1. **Orchestrator** designs the calling convention and register protocol
2. **Fixer/Coder** writes `.asm` file under `kernel/`
3. **Fixer/Coder** wires the Rust ↔ asm interface

### Testing
```bash
make        # Build the kernel (no debug output)
make DEBUG=1 # Build with debug output enabled
make run    # Run in QEMU (serial output)
make test   # Automated boot test
```

---

## **Critical Kernel Knowledge**

### 64-bit Mode iretq Always Pops SS:RSP
In IA-32e (long) mode, `iretq` **always** pops and validates SS:RSP — even for a
same-privilege-level return (CS.RPL == current CPL). This differs from 32-bit
mode where SS:RSP are only popped on CPL changes.

**Consequence**: When building a synthetic iretq frame for kernel-mode execution
(e.g., idle process), both **CS and SS must match the target CPL**:
- CPL=0: CS=0x08 (kernel code), SS=0x10 (kernel data)
- CPL=3: CS=0x23 (user code),  SS=0x1B (user data)

A mismatch (e.g., CS=0x08, SS=0x1B) causes **GPF #13** with error code pointing
at the SS selector. In our case: error=0x0018 = GDT index 3 = USER_DATA
descriptor.

**Relevant files**: `kernel_rust/src/process.rs` (spawn_init), `kernel/syscall_entry.asm`

### Register Frame Layout (kernel stack, offsets from kernel_rsp)
```
+168: SS         (0x1B for user, 0x10 for kernel idle)
+160: user RSP
+152: RFLAGS     (0x202 — IF set)
+144: CS         (0x23 for user, 0x08 for kernel idle)
+136: RIP
+128: err_code
+120: int_no     (0 for synthetic frames)
+112: R15
+104: R14
 +96: R13
 +88: R12
 +80: R11
 +72: R10
 +64: R9
 +56: R8
 +48: RDI
 +40: RSI
 +32: RBP
 +24: RBX
 +16: RDX
  +8: RCX
  +0: RAX        <-- kernel_rsp points here
```

`build_init_frame()` (process.rs) builds this frame from the top-of-stack down.
After construction, overrides for kernel-mode idle set CS=0x08 and SS=0x10.

### Debug Output System (`make DEBUG=1`)
A Cargo feature `debug` gates verbose debug prints:

- `Cargo.toml`: `[features] debug = []`
- `Makefile`: `DEBUG ?= 0`, passes `--features debug` when `DEBUG=1`
- Rust code: `if cfg!(feature = "debug") { ... }` — compiler optimizes away the
  block when not enabled (constant-folds `cfg!()` to `false`)

Usage:
- `make` → no debug output
- `make DEBUG=1` → verbose frame/stack dumps on exceptions + idle process details

To add new debug-only output, wrap it in `if cfg!(feature = "debug") { }`.
To delete all debug state and rebuild clean: `make clean && make DEBUG=1`.

### Common Debug Flow for New Exceptions
1. Add `if cfg!(feature = "debug") { ... }` block in the exception handler
2. Build with `make DEBUG=1`
3. Run with `make run`, capture serial output
4. Compare frame values against known valid state (see frame layout above)

---

## **Critical Kernel Knowledge — Phase 2 Additions**

### TLB Must Be Flushed After load_flat_binary
When `exec()` maps new pages via `load_flat_binary()`, the CPU may have stale
TLB entries from the previous process. Always call `paging::invlpg()` after
each `paging::map_4k()` call in `load_flat_binary()`. Without this, the CPU
may execute stale code from the old mapping after `sysretq`, causing
unpredictable crashes.

**Relevant files**: `kernel_rust/src/process.rs` (`load_flat_binary`)

### Blocking TTY Read Protocol
The TTY read is non-blocking — it returns 0 immediately when the ring buffer
is empty. For interactive shells, this causes busy-looping. Use the
`waiting_pid` pattern in `vfs/tty.rs`:
- `tty.read()`: if buffer empty, set `self.waiting_pid = current_pid()`,
  set process state to `Blocked`, set `should_schedule = 1`, return 0.
- `tty.push_byte()` or `process_input()`: after adding data, if
  `self.waiting_pid != 0`, find that process and set state to `Ready`,
  set `should_schedule = 1`.

### Initramfs Layout for `INIT=vibit`
When building with `INIT=vibit`:
- `sbin/init` = VIBIT init system (from `../vibit/vibit.bin`)
- `bin/vish` = vish shell (from `../vish/vish.bin`)
- `bin/{echo,cat,clear,false,printenv,true,yes}` = GVIBU coreutils
  (from `../gvibu-ai-lab/kernel/user_*.asm`)

VIBIT spawns vish via `fork()` + `exec("/bin/vish")`. The vish binary is a
flat NASM binary at USER_CODE_ADDR (0x2000000).

### `make test` Now Includes Anti-Plagiarism Check
```bash
make test   # Runs anti_cheat.py first, then test_kernel.py
make INIT=vibit run  # Interactive shell with VIBIT + vish + GVIBU
make INIT=vibit test # Same but automated (times out — shell waits for input)
```

### User/Userspace Separation
Source code for userspace programs lives in their own repos:
- `../vibit` — PID 1 init system (NASM)
- `../vish` — Cross-platform shell (Rust + NASM flat binary)
- `../gvibu-ai-lab` — Coreutils (Rust + NASM)

**Rules:**
1. **All source code changes** to VIBIT, vish, or GVIBU must be made in their
   respective repos (not in the VIBIX repo). Commit and push to their own
   feature branches, then file PRs to their `main` branches.
2. **Only binaries** (`.bin` or compiled ELFs) are copied to the VIBIX repo's
   `userspace/` directory for integration testing. The Makefile's `INIT=vibit`
   target handles this automatically — it runs `make` in each external repo
   and copies the output binary.
3. **The VIBIX kernel repo ships without userspace binaries.** In production,
   the kernel and userspace programs are packaged separately (distro model),
   just like other UNIXoid operating systems. The `userspace/` directory and
   its contents exist only for development and testing.
4. **Kernel source code** stays in `kernel_rust/src/` and `kernel/`. Never mix
   userspace source code into these directories.
5. **Anti-plagiarism**: The `anti_cheat.py` check runs as part of `make test`
   and scans ALL source files (including `.rs`, `.asm`, `.inc`) for
   Linux/BSD/GPL patterns. False positives for ABI compatibility references
   are handled via the exception system in `anti_cheat.py`.

### exec() Caveats
- `exec()` does NOT reset `sig_pending`. The child inherits the parent's
  signal mask. Currently fork() sets `sig_pending = 0` for the child, so
  exec'd processes start clean.
- `exec()` does NOT use argv/envp yet (the function signature accepts them
  but they're ignored).
- After `exec`, the syscall return path reads `syscall_state.rip/rsp/rflags`
  directly. These MUST be updated before the assembly return reads them.
  Use `core::ptr::write_volatile` to prevent the release-mode compiler from
  reordering or caching these writes.
- `syscall_entry.asm` now zeros `rdi`/`rsi`/`rdx` before `sysretq` to prevent
  leaking stale kernel or old-process pointers to the restored process.

### ELF Loader: Map RW Then Relax
The ELF loader (`elf.rs`) must map pages as writable first, copy segment data,
then relax to read-only. Mapping directly as read-only causes a page fault when
the loader tries to write segment data to the virtual address.

**NX bit caution**: `PAGE_NO_EXEC` (bit 63) causes a reserved-bit Page Fault
(#PF with ERR bit 3) unless `EFER.NXE` is enabled. Currently NX is not enabled,
so the ELF loader does NOT use the NX bit. All pages remain executable.

**Relevant files**: `kernel_rust/src/elf.rs`

### TCG Triple Fault: Idle Process RSP
The idle process's register frame (built by `build_init_frame`) had `user_rsp=0`.
In 64-bit mode, `iretq` always pops SS:RSP even for same-privilege returns.
With RSP=0, the first interrupt (PIT timer) tries to push to address 0 and
causes a page fault. On KVM this is masked by VM entry/exit TLB handling, but
on TCG it triggers a triple fault (page fault → double fault → triple fault).

**Fix**: Pass `idle_ktop` (a valid kernel stack address) as the RSP value when
building the idle process frame.

### `make test_vibit`
```bash
make test_vibit   # Build with INIT=vibit, run 7 extra checks
```
Checks fork, exec, waitpid, blocking TTY read via VIBIT + vish markers.

---

## **Next Session: Per-Process Page Tables + vish Rust Frontend**

### Per-Process Page Tables (P₃)
Currently all processes share the same page tables (single address space).
To implement proper process isolation:

1. **Add `pml4` field to `Process` struct** — each process has its own PML4 table.
2. **Initialize PML4 at fork/spawn** — copy kernel mappings, create new user mapping.
3. **Switch CR3 on context switch** — in `scheduler_tick()` and `scheduler_switch_exit()`.
4. **Update `load_flat_binary` and ELF loader** — map pages into the process's own page tables instead of the global ones.
5. **Fix `tty_wake` and cross-process IPI** — blocking read wake-up needs the target process's page tables.

**Key challenge**: VIBIX currently modifies the ACTIVE page tables directly.
With per-process tables, the kernel must either:
- Map all process page tables into the kernel's address space (e.g., at a fixed
  virtual address range like `0xFFFF8000_00000000`) for modification, or
- Temporarily switch CR3 to the target process's tables when modifying them.

**Suggested approach**: Reserve a fixed virtual address range in the kernel for
accessing process page tables (recursive mapping). This avoids expensive CR3
switches.

### vish Rust Frontend on VIBIX
vish has a cross-platform Rust core (`src/lib.rs`, `src/parse.rs`, `src/readline.rs`,
`src/exec.rs`) with platform backends:
- `src/linux/` — Linux ELF backend (uses libc)
- `src/vibix/` — VIBIX flat binary backend (currently just `vish.asm`)

To port the Rust frontend:

1. **Build Rust for VIBIX** — vish's Rust core compiles with `#![no_std]` for VIBIX.
   It needs a minimal platform layer (`src/vibix/mod.rs`) providing:
   - `sys_read(fd, buf, len)` → syscall 2
   - `sys_write(fd, buf, len)` → syscall 1
   - `sys_fork()` → syscall 8
   - `sys_exec(path, argv, envp)` → syscall 9
   - `sys_waitpid(pid, wstatus, flags)` → syscall 10
   - `sys_open(path, flags)` → syscall 12 (for PATH-based command search)
   - `sys_isatty(fd)` → syscall 24
   - `sys_tcgetattr(fd, termios)` → syscall 25
   - `sys_tcsetattr(fd, termios)` → syscall 26
2. **Link as ELF** — The Rust compiler produces ELF, which the kernel's ELF loader
   already supports (with the RW-then-relax fix).

**Relevant repos**: `../vish` (source), `../gvibu-ai-lab/vibix-lib/` (reference for
Rust ELF build setup for VIBIX).

--
