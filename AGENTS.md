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
make        # Build the kernel
make run    # Run in QEMU (serial output)
make test   # Automated boot test
```
--
