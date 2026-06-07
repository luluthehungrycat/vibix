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

**Current status: MVP boots in QEMU, prints serial output, and manages physical
memory — built entirely in Rust (stable, no_std).**

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
| **Kernel core** (serial, PMM, idle) | **Rust (stable)** | `kernel_rust/src/` |
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

### Next Up (Recommended Order)

This ordering respects technical dependencies:

1. **Interrupts (IDT + PIC remapping + IRQ handlers)** — unlocks every I/O
   subsystem. The keyboard, timer, and other drivers are all interrupt-driven.
   Nothing else can happen without this.
2. **Programmable Interval Timer (PIT)** — the OS heartbeat. Gives us time
   tracking and a tick for scheduling.
3. **PS/2 Keyboard driver** — first real input. Needs interrupts (IRQ1).
   Simple protocol makes it a good first driver.
4. **Framebuffer / VBE display** — visual output on QEMU's serial console or
   Bochs VBE LFB. Can be developed in parallel with keyboard.
5. **Paging enhancements** — demand paging, recursive page tables, virtual
   memory allocator. Largely independent of I/O; can proceed alongside 3–4.

After these are stable, the path opens to:
- **Shell** — keyboard + serial → interactive input
- **Syscalls** — enter/exit kernel mode
- **Preemptive multitasking** — timer + IRQ → context switching
- **File systems** — block device interface

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
PMM: init 0x100000-0x10000000 (65280 pages)
PMM: Test passed.
VIBIX: Boot sequence complete — entering idle loop.
```

---

## Tools

| Script | Purpose |
|--------|---------|
| `anti_cheat.py` | Scans for Linux/BSD kernel patterns to prevent accidental plagiarism |
| `test_kernel.py` | Boots QEMU, captures serial output, verifies boot banner |
| `run_qemu.sh` | QEMU runner with debug logging (`-d int,cpu_reset`) |

---

## Development

### Coding Philosophy

- **Originality first** — every line is handwritten; no copying from existing kernels
- **Rust safety** — leverage the type system for memory safety at compile time
- **Serial-driven debugging** — no graphical output needed for development
- **Testable by default** — every feature should have an automated test

### Anti-Plagiarism

Run `python3 anti_cheat.py` to check for forbidden patterns from Linux, BSD, and
GPL-licensed kernels. The scanner checks `.c`, `.h`, `.asm`, and `.S` files for
known kernel idioms.

---

*Logo: TBD*
