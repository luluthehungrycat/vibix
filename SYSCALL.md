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
| 2 | `read`   | `ssize_t read(int fd, void *buf, size_t len)`    | ⚠️ Stub |
| 3 | `getpid` | `pid_t getpid(void)`                             | ✅     |

Slots 4–63 are reserved for future expansion.

---

## Return Values and Errors

Currently, VIBIX does **not** set an `errno` or use a separate error flag.

- **Success:** Returns a non-negative value (typically `>= 0`).
- **Error:** Returns `u64::MAX` (`0xFFFF_FFFF_FFFF_FFFF` = `-1` when treated as
  `ssize_t` or `pid_t`).
- **Unknown syscall number:** Returns `u64::MAX`.

Each syscall defines its own specific return convention (see
[Syscall Reference](#syscall-reference)).

Upcoming: a proper `errno` mechanism and a set of standard error codes
(see [Future / Planned Syscalls](#future--planned-syscalls)).

---

## Executable Format

VIBIX currently supports **flat binaries only** — output of `nasm -f bin`.
Programs are loaded at virtual address `0x2000000` (32 MiB).  The first byte of
the binary is the first instruction executed (equivalent to `_start`).

**ELF support is planned** (top priority for userspace program portability).

### Building a flat binary

```makefile
my_program.bin: my_program.asm
    nasm -f bin $< -o $@
```

The binary is embedded at kernel compile time via `include_bytes!` in the Rust
source.  Replacing the init program currently requires editing
`kernel_rust/src/process.rs`.

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

**⚠️ Status:** Stub.  Currently always returns `-1` (`u64::MAX`).
A working implementation (keyboard input for `fd=0`) is in development.

**Arguments:**
- `rdi` = `fd`: File descriptor.
- `rsi` = `buf`: Pointer to buffer in user address space.
- `rdx` = `count`: Maximum number of bytes to read.

**Return:** Number of bytes read, or `-1` on error (always).

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

---

## Future / Planned Syscalls

| # | Name      | Priority | Notes                                  |
|---|-----------|----------|----------------------------------------|
| 4 | `open`    | Medium   | Open a file                            |
| 5 | `close`   | Medium   | Close a file descriptor                |
| 6 | `brk`     | High     | Change heap size (for `malloc`)        |
| 7 | `sbrk`    | High     | Incremental heap growth                |
| 8 | `ioctl`   | Low      | Device I/O control                     |
| 9 | `mmap`    | Medium   | Memory-mapped files / anonymous maps   |

Additional planned features:
- **`errno`**: a global `errno` variable or thread-local storage for error codes.
- **`fork`/`exec`**: process creation and program loading.
- **`waitpid`**: process synchronization.
- **`exit_group`**: exit all threads in a process group.

---

## Change Log

| Date       | Version | Changes                                     |
|------------|---------|---------------------------------------------|
| 2026-06-22 | 1.0     | Initial specification — 4 syscalls, flat binary format |
