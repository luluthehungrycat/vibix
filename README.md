# VIBIX

Minimal UNIX-like kernel developed from scratch using a Rust + assembly codebase
and coding agent workflows (OpenCode, Vibe CLI, Hermes Agent).
Targets QEMU/KVM first.

---

## Overview

VIBIX is a pedagogical kernel project with a twist — it's built primarily through
coding agent collaboration alongside traditional coding. The kernel was originally
prototyped in freestanding C, then ported to Rust for stronger safety guarantees
and better alignment with modern kernel development practices.

**Current status: MVP boots in QEMU with full interrupt handling (IDT, PIC, PIT
at 100 Hz), VBE framebuffer (1024×768×32 with bitmap font rendering), physical
memory manager, kernel heap allocator, keyboard driver module with ring buffer,
GDT with Ring 3 segments and TSS, SYSCALL/SYSRET entry (via LSTAR MSR), syscall
dispatch table (5 syscalls: exit, write, read, getpid, brk), ELF64 loader,
and kernel-internal errno — all built in Rust (stable, no_std).  The kernel
enters user mode (Ring 3), runs a flat binary init process (PID 1) that prints
"Hello, world!" and "From PID 1 (init)", then exits with code 0.**

---

## Architecture

### Two-Stage Build

```
boot.asm (ELF32, Multiboot v1) ──incbin──► kernel64.bin (flat 64-bit binary)
                                                │
                                           ┌────┴────┐
                                           │ Rust    │
                                           │ no_std  │
                                           │ kernel  │
                                           └────┬────┘
                                                │
                                          kernel64_elf.ld
                                          (link at 0x200000)
```

| Stage | Format | Entry | What it does |
|-------|--------|-------|-------------|
| `boot.asm` | ELF32 | `_start` (MB1 entry) | Sets up GDT, PML4, enters long mode, `incbin`s kernel64.bin, calls `kernel_main` |
| `kernel64.bin` | Flat binary | `_kernel64_start` | Rust kernel: serial driver, PMM bitmap allocator, idle loop |

### Address Space Layout (boot stage)

```
0x001000  ┌──────────────────────────────┐
          │  32-bit ELF (Multiboot)       │ ← boot.asm + boot.o
0x100000  ┌──────────────────────────────┐
          │  PML4 / Page Tables (4 pages) │ ← identity-map first 4 MiB
0x200000  ┌──────────────────────────────┐
          │  Kernel (64-bit flat binary)  │
0x200A40  │  Stack (16 KiB)              │
0x204A40  │  PMM Bitmap (~8 KiB)         │
0x206A54  └──────────────────────────────┘
```

### Language Split

| Layer | Language | Location |
|-------|----------|----------|
| Multiboot header + mode transition | Assembly (NASM) | `boot.asm` |
| 64-bit kernel entry stub | Assembly (NASM) | `kernel/kernel64_entry.asm` |
| Syscall entry stub | Assembly (NASM) | `kernel/syscall_entry.asm` |
| User-mode init process (flat binary) | Assembly (NASM) | `kernel/user_init.asm` |
| **Kernel core** (serial, PMM, KMM, interrupts, PIT, keyboard, framebuffer, GDT, paging, syscall dispatch, process) | **Rust (stable)** | `kernel_rust/src/` |
| Legacy C implementation (archived) | C (GCC) | `legacy/c_kernel/` |

---

## Progress

### Implemented

- [x] **Multiboot v1 boot** — QEMU `-kernel` loads our ELF32
- [x] **32→64-bit transition** — GDT, PAE, PML4, long mode entry
- [x] **Two-stage build system** — flat 64-bit binary `incbin`ed into ELF32 wrapper
- [x] **Serial output** — COM1 at 115200 8N1 via port I/O (Rust)
- [x] **Physical Memory Manager** — bitmap allocator, 256 MB managed, self-test (Rust)
- [x] **Rust port** — kernel core migrated from C to Rust (stable, `no_std`, `x86_64-unknown-none` target)
- [x] **Automated test suite** — `make test` boots QEMU and validates output
- [x] **Anti-plagiarism checks** — `python3 anti_cheat.py` scans for Linux/BSD patterns
- [x] **Interrupts (IDT + PIC)** — remapped IRQs, handler dispatch (Rust+asm)
- [x] **PIT driver** — 100 Hz system timer via IRQ0
- [x] **PS/2 Keyboard driver** — IRQ1 handler, scancode decoding
- [x] **Framebuffer display** — Bochs VBE direct I/O, 1024×768×32, 8×16 bitmap font
- [x] **Kernel heap allocator** — first-fit free-list, coalescing, split
- [x] **GDT with Ring 3 segments + TSS** — user-mode code/data segments and task
  state segment for syscall stack switching.
- [x] **Syscall entry** — `syscall`/`sysretq` handler via LSTAR MSR, saves/restores
  user registers, dispatches to Rust.
- [x] **Syscall dispatch** — 64-slot table, 5 syscalls registered (exit, write,
  read, getpid, brk).  ABI documented in [SYSCALL.md](SYSCALL.md).
- [x] **User-mode entry** — flat binary init process (PID 1) loaded at 32 MiB,
  entered via IRETQ to Ring 3, prints "Hello, world!" and exits cleanly with
  code 0.
- [x] **Paging** — 4-level page tables, 4 KiB and 2 MiB pages, map/unmap/translate,
  self-test.
- [x] **`read` syscall** — PS/2 keyboard driver wired to `sys_read` (fd=0).
- [x] **`brk` syscall** — dynamic heap growth with demand paging (up to 256 MiB).
- [x] **ELF64 loader** — parses `ET_EXEC` with `PT_LOAD` segments, creates user pages.
- [x] **`errno` mechanism** — kernel-internal `errno` set on syscall errors.
- [x] **Correct syscall ABI** — register rotation in `syscall_entry.asm` preserves
  all 5 user arguments (verified: exit code 0).

### Next Up (Recommended Order)

1. **Multiple processes + scheduler** — context switching, preemptive
   multitasking, `fork`/`exec`.
2. **Shell** — keyboard + serial + framebuffer → interactive user interface.
3. **File systems** — block device interface, initrd/tmpfs.

---

## Build & Run

### Prerequisites

- `nasm` — assembler
- `make` — build system
- `cargo` + `rustc` — Rust compiler (stable)
- `qemu-system-x86_64` — emulator (with TCG or KVM)
- `python3` — test runner

The Rust target `x86_64-unknown-none` is installed automatically by the build.

### Commands

```bash
# Build the kernel
make

# Run in QEMU (serial output to terminal)
make run

# Run automated test
make test

# Debug with GDB (QEMU waits at -s -S)
make debug
```

### Output

A successful boot prints:

```
========================================
  VIBIX — UNIXoid Kernel (Rust Port)
========================================

VIBIX: Kernel alive!
VIBIX: Multiboot memory map:
  base=0x00000000_0x00000000  len=0x00000000_0x0009FC00  Available
  base=0x00000000_0x0009FC00  len=0x00000000_0x00000400  Reserved
  base=0x00000000_0x000F0000  len=0x00000000_0x00010000  Reserved
  base=0x00000000_0x00100000  len=0x00000000_0x1FEE0000  Available
  base=0x00000000_0x1FFE0000  len=0x00000000_0x00020000  Reserved
  base=0x00000000_0xFFFC0000  len=0x00000000_0x00040000  Reserved
  base=0x000000FD_0x00000000  len=0x00000003_0x00000000  Reserved
PMM: Test passed.
KMM: Test OK — freed block reused.
KMM: Coalescing OK — single free block.
PAGING: Test passed.
VIBIX: fb: Bochs VBE detected — setting 1024x768x32.
VIBIX: fb: VBE status — 1024x768x32 enable=0xc1
VIBIX: Framebuffer: 1024 x 768 @ 32 bpp, addr fd000000
VIBIX: Framebuffer initialised.
VIBIX: Initialising interrupts...
VIBIX: IDT loaded, PIC remapped.
VIBIX: PIT timer initialised at 100 Hz.
VIBIX: Loading GDT/TSS and enabling SYSCALL.
VIBIX: Enabling interrupts.
VIBIX: Boot sequence complete — spawning PID 1.
VIBIX: Entering user mode...
Hello, world!
From PID 1 (init)
VIBIX: init exited with code 0
```

---

## Tools

| Script | Purpose |
|--------|---------|
| `anti_cheat.py` | Scans for Linux/BSD kernel patterns to prevent accidental plagiarism |
| `test_kernel.py` | Boots QEMU, captures serial output, verifies boot banner |
| `run_qemu.sh` | QEMU runner with debug logging (`-d int,cpu_reset`) |
| `SYSCALL.md` | Formal system call ABI specification for userspace programs |

---

## Development

### Coding Philosophy

- **Originality first** — every line is handwritten; no copying from existing kernels
- **Rust safety** — leverage the type system for memory safety at compile time
- **Serial-driven debugging** — primary debug channel even with graphical output
- **Testable by default** — every feature should have an automated test

### Anti-Plagiarism

Run `python3 anti_cheat.py` to check for forbidden patterns from Linux, BSD, and
GPL-licensed kernels. The scanner checks `.c`, `.h`, `.asm`, and `.S` files for
known kernel idioms.

---

*Logo: TBD*
