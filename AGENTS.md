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

Only binaries are copied to `userspace/` for testing. The VIBIX repo ships
without these binaries — they're packaged separately in a distro model.

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

--
