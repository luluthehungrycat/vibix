# VIBIX Syscall ABI Specification

**Version:** 1.0  
**Status:** Stable (expandable)  
**Last updated:** 2026-06-22  

This document defines the VIBIX system call interface — the contract between
user-mode programs and the kernel.  Any program targeting VIBIX must follow
these conventions.

---

## Table of Contents

- [Calling Convention](#calling-convention)
- [Syscall Numbers](#syscall-numbers)
- [Return Values and Errors](#return-values-and-errors)
- [Executable Format](#executable-format)
- [Process Model](#process-model)
- [Memory Layout](#memory-layout)
- [Syscall Reference](#syscall-reference)
  - [0 — exit](#0--exit)
  - [1 — write](#1--write)
  - [2 — read](#2--read)
  - [3 — getpid](#3--getpid)
- [Future / Planned Syscalls](#future--planned-syscalls)
- [Change Log](#change-log)

---

## Calling Convention

Syscalls use the `syscall` instruction (LSTAR MSR, Ring 3 → Ring 0).

### Argument registers

| Register | Role                                   |
|----------|----------------------------------------|
| `rax`    | Syscall number                         |
| `rdi`    | arg1                                   |
| `rsi`    | arg2                                   |
| `rdx`    | arg3                                   |
| `r8`     | arg4                                   |
| `r9`     | arg5 *(wired in dispatch but ignored — planned for future)* |

### Return register

| Register | Role         |
|----------|--------------|
| `rax`    | Return value |

### Preserved registers

The kernel saves and restores `rcx` and `r11` (clobbered by `syscall`/`sysretq`).
**All other registers** (including `rbx`, `rbp`, `r12`–`r15`, `rsi`, `rdi`,
`rdx`, `r8`, `r9`, `r10`) are **clobbered** — the kernel does not save or
restore them on syscall entry/exit.  Userspace must treat every syscall as a
full register clobber.

> **Note for compilers:** When generating syscall wrappers, save any live
> registers before `syscall` and restore them after.

### Example

```asm
; write(1, msg, 15)
mov rax, 1          ; syscall number
mov rdi, 1          ; arg1: fd = stdout
lea rsi, [rel msg]  ; arg2: buffer
mov rdx, 15         ; arg3: length
syscall             ; ⟶ rax = bytes written
```

---

## Syscall Numbers

| # | Name     | Signature                                        | Status |
|---|----------|--------------------------------------------------|--------|
| 0 | `exit`   | `void exit(int code)`                            | ✅     |
| 1 | `write`  | `ssize_t write(int fd, const void *buf, size_t len)` | ✅  |
| 2 | `read`   | `ssize_t read(int fd, void *buf, size_t len)`    | ✅ Keyboard |
| 3 | `getpid` | `pid_t getpid(void)`                             | ✅     |
| 4 | `brk`    | `int brk(void *addr)`                            | ✅     |

Slots 5–63 are reserved for future expansion.

---

## Return Values and Errors

- **Success:** Returns a non-negative value (typically `>= 0`).
- **Error:** Returns `u64::MAX` (`0xFFFF_FFFF_FFFF_FFFF` = `-1` when treated as
  `ssize_t` or `pid_t`).
- **Unknown syscall number:** Returns `u64::MAX`.

A kernel-internal `errno` value is maintained and set by syscalls on error.
Userspace can retrieve it via the exported kernel function `get_errno()` (planned:
a dedicated `sys_errno` syscall or TLS-based access).

### Error codes

| Code | Name     | Meaning                              |
|------|----------|--------------------------------------|
| 38   | `ENOSYS` | Function not implemented / out of memory (temporary stand-in) |

Each syscall defines its own specific return convention (see
[Syscall Reference](#syscall-reference)).

---

## Executable Format

VIBIX supports:

- **Flat binaries** — output of `nasm -f bin`. The first byte of the binary is
  the first instruction executed (equivalent to `_start`). This is the format
  used for the built-in init program (PID 1).
- **ELF64 executables** — standard `ET_EXEC` files are loaded by the kernel's
  ELF loader (`kernel_rust/src/elf.rs`). The loader parses `PT_LOAD` segments,
  allocates physical pages, maps them at the requested virtual addresses with
  appropriate permissions (user-accessible, writable when `PF_W` is set,
  non-executable when `PF_X` is clear), and zero-fills BSS.

### Building for VIBIX

**Flat binary (init):**
```makefile
my_program.bin: my_program.asm
    nasm -f bin $< -o $@
```

The init binary is embedded at kernel compile time via `include_bytes!` in
`kernel_rust/src/process.rs`.  Replacing it requires editing that file.

**ELF64 executable (third-party programs):**
```makefile
my_program: my_program.asm
    nasm -f elf64 $< -o my_program.o
    ld -o my_program my_program.o
```

ELF executables can be loaded by the kernel at runtime via `create_from_elf()`.

---

## Process Model

VIBIX currently runs a **single process** (PID 1, the init program).

- No `fork`, `exec`, or `clone` — only one process exists.
- No preemptive multitasking — the PIT timer fires but there is no scheduler.
- No context switching — `enter_user_mode()` never returns.
- When `exit()` is called, the kernel prints a message and halts the CPU.

Planned: preemptive multitasking with a round-robin or priority scheduler.

---

## Memory Layout

### User address space

```
0x2000000  ┌──────────────────┐  ← _start (entry point)
           │  code + rodata   │  ← 4 KiB page (user r/w)
0x2001000  ├──────────────────┤
           │  user stack      │  ← 4 KiB page (user r/w), grows downward
0x2002000  └──────────────────┘  ← initial RSP
```

- Both pages have `PAGE_USER_RW` (present + writable + user-accessible).
- There are **no guard pages** — stack overflow will silently corrupt the code
  page below.
- The stack is **4 KiB total** — deep call chains or large stack allocations
  will overflow.

### User heap (brk area)

```
0x201_0000  ┌──────────────────┐  ← initial program break
            │  heap (brk)      │  ← dynamically allocated pages
            ⋮                  ⋮
            ⋮                  ⋮
0x1000_0000 └──────────────────┘  ← BRK_MAX (256 MiB)
```

The heap is managed by the `brk` syscall (#4). Pages are allocated and mapped
on demand, zeroed to prevent kernel-data leaks.

### Kernel address space

The kernel resides at `0x200000` (2 MiB) and is mapped with supervisor-only
pages.  Userspace code cannot access kernel memory.

---

## Syscall Reference

### 0 — exit

```c
void exit(int code);
```

**Description:** Terminates the current process.  In the current
single-process implementation, this halts the entire system after printing a
message.

**Arguments:**
- `rdi` = `code`: Exit status (convention: `0` = success).

**Return:** Never returns.

**Example:**
```asm
mov rax, 0
mov rdi, 0    ; code = 0 (success)
syscall
```

---

### 1 — write

```c
ssize_t write(int fd, const void *buf, size_t count);
```

**Description:** Writes up to `count` bytes from `buf` to the file descriptor
`fd`.  Currently only `fd=1` (stdout) is supported, mapped to the serial
console (COM1 at 115200 8N1).

**Arguments:**
- `rdi` = `fd`: File descriptor (`1` = stdout).
- `rsi` = `buf`: Pointer to buffer in user address space.
- `rdx` = `count`: Number of bytes to write.

**Return:** Number of bytes written on success, `0` for unsupported fd.

**Example:**
```asm
mov rax, 1
mov rdi, 1
lea rsi, [rel message]
mov rdx, 13
syscall          ; ⟶ rax = 13
```

---

### 2 — read

```c
ssize_t read(int fd, void *buf, size_t count);
```

**Description:** Reads up to `count` bytes from file descriptor `fd` into
`buf`.

- **`fd=0` (stdin):** Reads from the PS/2 keyboard circular buffer. This is a
  **non-blocking** read — returns immediately with whatever keystrokes are
  available (may return 0 if the buffer is empty).
- **Other fds:** Returns `-1` (unsupported).

The keyboard driver (`kernel_rust/src/keyboard.rs`) decodes scan code set 1
into ASCII, buffers keystrokes in a 256-byte ring buffer, and echoes them to
the serial console during the interrupt handler.

**Arguments:**
- `rdi` = `fd`: File descriptor (`0` = stdin).
- `rsi` = `buf`: Pointer to buffer in user address space.
- `rdx` = `count`: Maximum number of bytes to read.

**Return:** Number of bytes read, or `-1` for unsupported fd.

---

### 3 — getpid

```c
pid_t getpid(void);
```

**Description:** Returns the process ID of the calling process.

**Arguments:** None.

**Return:** Process ID (always `1` in the current single-process kernel).

**Example:**
```asm
mov rax, 3
syscall          ; ⟶ rax = 1
```

### 4 — brk

```c
int brk(void *addr);
```

**Description:** Changes the program break (end of the data segment / start of
the heap).

- **`addr == 0` (brk(0) / sbrk(0)):** Returns the current program break address
  without changing it.
- **`addr < BRK_START` (0x201_0000) or `addr > BRK_MAX` (0x1000_0000):**
  Returns `-1` and sets `errno` to `ENOSYS` (no change).
- **Otherwise:** Sets the new break. If the break moves past the last mapped
  page, the kernel allocates and maps new pages (zeroed, user r/w). Shrinking
  the break does not unmap pages (for simplicity).

**Arguments:**
- `rdi` = `addr`: New program break address, or `0` to query.

**Return:** The new (or current) program break address on success, or `-1` on
error.

**Example:**
```asm
; Query current break (sbrk(0))
mov rax, 4
mov rdi, 0
syscall          ; ⟶ rax = current break (e.g., 0x2010000)
```

---

## Future / Planned Syscalls

| # | Name      | Priority | Notes                                  |
|---|-----------|----------|----------------------------------------|
| 5 | `open`    | Medium   | Open a file                            |
| 6 | `close`   | Medium   | Close a file descriptor                |
| 7 | `sbrk`    | Low      | Incremental heap growth (brk covers it)|
| 8 | `ioctl`   | Low      | Device I/O control                     |
| 9 | `mmap`    | Medium   | Memory-mapped files / anonymous maps   |

Additional planned features:
- **`fork`/`exec`**: process creation and program loading.
- **`waitpid`**: process synchronization.
- **`exit_group`**: exit all threads in a process group.
- **TLS-based `errno`**: thread-local storage for per-process error codes.

---

## Change Log

| Date       | Version | Changes                                                |
|------------|---------|--------------------------------------------------------|
| 2026-06-22 | 1.1     | Added brk (#4), keyboard-backed read (#2), errno, ELF loader |
