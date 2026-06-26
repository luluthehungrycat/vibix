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

**Current status: MVP boots in QEMU with full preemptive multitasking (round-robin
scheduler, 64-process table, idle process), VFS layer (vnode abstraction, open
file table, mount table, devfs, initramfs with ustar parser, rootfs), 19 syscalls
(exit, write, read, getpid, brk, nanosleep, uname, reboot, fork, exec, waitpid,
mmap, open, close, lseek, getdents, dup, dup2, pipe), ELF64 + flat binary loader,
per-process fd table with /dev/ttyS0 for fd 0/1/2, and full interrupt handling
(IDT, PIC, PIT at 100 Hz), VBE framebuffer, PMM, KMM, keyboard driver, GDT with
Ring 3/TSS — all built in Rust (stable, no_std).**

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

#### Core Infrastructure
- [x] **Multiboot v1 boot** — QEMU `-kernel` loads our ELF32
- [x] **32→64-bit transition** — GDT, PAE, PML4, long mode entry
- [x] **Two-stage build system** — flat 64-bit binary `incbin`ed into ELF32 wrapper
- [x] **Serial output** — COM1 at 115200 8N1 via port I/O (Rust)
- [x] **Physical Memory Manager** — bitmap allocator, 256 MB managed, self-test (Rust)
- [x] **Kernel heap allocator** — first-fit free-list, coalescing, split
- [x] **Automated test suite** — `make test` boots QEMU and validates output
- [x] **Anti-plagiarism checks** — `python3 anti_cheat.py` scans for Linux/BSD patterns

#### Interrupts & Devices
- [x] **Interrupts (IDT + PIC)** — remapped IRQs, handler dispatch (Rust+asm)
- [x] **PIT driver** — 100 Hz system timer via IRQ0
- [x] **PS/2 Keyboard driver** — IRQ1 handler, scancode decoding
- [x] **Framebuffer display** — Bochs VBE direct I/O, 1024×768×32, 8×16 bitmap font

#### Memory Management
- [x] **Paging** — 4-level page tables, 4 KiB and 2 MiB pages, map/unmap/translate, self-test
- [x] **`brk` syscall** — dynamic heap growth with demand paging (up to 256 MiB)
- [x] **`mmap` syscall** — MAP_ANONYMOUS memory mapping (syscall 11)

#### Process & Scheduler
- [x] **Preemptive round-robin scheduler** — PIT-driven context switching, 64-slot process table
- [x] **Per-process kernel stacks** — 4 KiB per process, switched on context switch
- [x] **Idle process** — PID 2, HLT loop when nothing else is ready
- [x] **Process states** — Ready, Running, Blocked, Zombie with correct transitions
- [x] **`fork` syscall** — creates child with copy of kernel stack, shared fd table (syscall 8)
- [x] **`exec` syscall** — ELF64 and flat binary loading from VFS path (syscall 9)
- [x] **`waitpid` syscall** — block parent until child exit, reap zombies (syscall 10)

#### Virtual File System
- [x] **Vnode abstraction** — Vnode with function-pointer ops table, FsType dispatch
- [x] **Global Open File Table** — 64 entries with refcounted OpenFile descriptors
- [x] **Per-process fd table** — 16 fds, embedded in Process struct, fork shares correctly
- [x] **Mount table** — static 4-slot, rootfs at `/`, devfs at `/dev`
- [x] **devfs** — `/dev/null`, `/dev/zero`, `/dev/ttyS0` with char device semantics
- [x] **initramfs** — embedded ustar archive parser, boot-time entry build, linear lookup
- [x] **rootfs** — minimal in-memory directory skeleton
- [x] **Path resolution** — component-by-component walk with mount point crossing
- [x] **`open`/`close` syscalls** — VFS-backed fd allocation (syscalls 12-13)
- [x] **`read`/`write` VFS dispatch** — fd→OFT→vnode→ops chain (syscalls 14-15)
- [x] **`lseek` syscall** — file offset repositioning (syscall 16)
- [x] **`getdents` syscall** — directory listing (syscall 17)

#### Additional Syscalls
- [x] **`exit` syscall** — Zombie state, wakes blocked parent, reschedules (syscall 0)
- [x] **`write` syscall** — serial console via /dev/ttyS0 (syscall 1)
- [x] **`read` syscall** — keyboard ring buffer via /dev/ttyS0 (syscall 2)
- [x] **`getpid` syscall** — returns current_pid() (syscall 3)
- [x] **`nanosleep` syscall** — busy-wait based on PIT ticks (syscall 5)
- [x] **`uname` syscall** — system identification structure (syscall 6)
- [x] **`reboot` syscall** — QEMU ISA reboot/poweroff (syscall 7)
- [x] **`dup`/`dup2` syscalls** — file descriptor duplication (syscalls 18-19)
- [x] **`pipe` syscall** — inter-process communication via shared ring buffer (syscall 20)

#### GDT / Syscall ABI
- [x] **GDT with Ring 3 segments + TSS** — user-mode code/data segments and TSS for syscall stack switching
- [x] **TSS.RSP0 update** — per-context-switch RSP0 for correct syscall kernel stack
- [x] **Syscall entry** — `syscall`/`sysretq` via LSTAR MSR, saves/restores user registers
- [x] **Syscall dispatch** — 64-slot table, 19 syscalls registered
- [x] **Correct syscall ABI** — register rotation preserves all 5 user arguments
- [x] **`errno` mechanism** — per-process errno set on syscall errors
- [x] **ELF64 loader** — parses `ET_EXEC` with `PT_LOAD` segments
- [x] **User-mode entry** — init process loaded from initramfs `/sbin/init`

### Next Up (Recommended Order)

See [docs/phase3c.md](docs/phase3c.md) for detailed planning on the next
development phase (Phase 3c: Shell, init, and ANSI terminal).

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
# Build the kernel (no debug output)
make

# Build with debug output (extra frame/stack dumps on exceptions)
make DEBUG=1

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
VIBIX: PS/2 keyboard ready.
VIBIX: Keyboard IRQ unmasked -- PS/2 input active.
VIBIX: Loading GDT/TSS and enabling SYSCALL.
VIBIX: Initialising VFS...
VIBIX: VFS ready.
VIBIX: Enabling interrupts.
VIBIX: Boot sequence complete — spawning PID 1.
VIBIX: Created PID 1 (init).
VIBIX: Starting scheduler...
<init process output>
```

---

## Tools

| Script | Purpose |
|--------|---------|
| `anti_cheat.py` | Scans for Linux/BSD kernel patterns to prevent accidental plagiarism |
| `test_kernel.py` | Boots QEMU, captures serial output, verifies boot banner |
| `run_qemu.sh` | QEMU runner with debug logging (`-d int,cpu_reset`) |
| `SYSCALL.md` | Formal system call ABI specification for userspace programs |
| `docs/scheduler-design.md` | Scheduler and multi-process architecture |
| `docs/vfs-design.md` | Virtual File System architecture |
| `docs/phase3c.md` | Phase 3c roadmap: shell, terminal, next steps |

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
