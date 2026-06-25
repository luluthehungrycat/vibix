# VIBIX Syscall ABI Specification

**Version:** 2.0  
**Status:** Stable  
**Last updated:** 2026-06-25  

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
  - [4 — brk](#4--brk)
  - [5 — nanosleep](#5--nanosleep)
  - [6 — uname](#6--uname)
  - [7 — reboot](#7--reboot)
  - [8 — fork](#8--fork)
  - [9 — exec](#9--exec)
  - [10 — waitpid](#10--waitpid)
  - [11 — mmap](#11--mmap)
  - [12 — open](#12--open)
  - [13 — close](#13--close)
  - [14 — read (VFS)](#14--read-vfs)
  - [15 — write (VFS)](#15--write-vfs)
  - [16 — lseek](#16--lseek)
  - [17 — getdents](#17--getdents)
  - [18 — dup](#18--dup)
  - [19 — dup2](#19--dup2)
  - [20 — pipe](#20--pipe)
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
| `r9`     | arg5 *(wired in dispatch but ignored)* |

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

| #  | Name       | Signature                                                     | Status |
|----|------------|---------------------------------------------------------------|--------|
| 0  | `exit`     | `void exit(int code)`                                         | ✅     |
| 1  | `write`    | `ssize_t write(int fd, const void *buf, size_t len)`          | ✅ VFS |
| 2  | `read`     | `ssize_t read(int fd, void *buf, size_t len)`                 | ✅ VFS |
| 3  | `getpid`   | `pid_t getpid(void)`                                          | ✅     |
| 4  | `brk`      | `int brk(void *addr)`                                         | ✅     |
| 5  | `nanosleep`| `int nanosleep(unsigned int sec, unsigned int nsec)`          | ✅     |
| 6  | `uname`    | `int uname(struct utsname *buf)`                              | ✅     |
| 7  | `reboot`   | `int reboot(int magic, int magic2, int cmd)`                  | ✅     |
| 8  | `fork`     | `pid_t fork(void)`                                            | ✅     |
| 9  | `exec`     | `int exec(const char *path, char *const argv[], char *const envp[])` | ✅ |
| 10 | `waitpid`  | `pid_t waitpid(pid_t pid, int *wstatus, int flags)`           | ✅     |
| 11 | `mmap`     | `void *mmap(void *addr, size_t length, int prot, int flags, int fd, off_t offset)` | ✅ Anonymous |
| 12 | `open`     | `int open(const char *path, int flags, ...)`                  | ✅     |
| 13 | `close`    | `int close(int fd)`                                           | ✅     |
| 14 | `read`     | `ssize_t read(int fd, void *buf, size_t len)`                 | ✅ VFS |
| 15 | `write`    | `ssize_t write(int fd, const void *buf, size_t len)`          | ✅ VFS |
| 16 | `lseek`    | `off_t lseek(int fd, off_t offset, int whence)`               | ✅     |
| 17 | `getdents` | `int getdents(int fd, struct dirent *dirp, int count)`        | ✅     |
| 18 | `dup`      | `int dup(int oldfd)`                                          | ✅     |
| 19 | `dup2`     | `int dup2(int oldfd, int newfd)`                              | ✅     |
| 20 | `pipe`     | `int pipe(int pipefd[2])`                                     | ✅     |

Slots 21–63 are reserved for future expansion.

---

## Return Values and Errors

- **Success:** Returns a non-negative value (typically `>= 0`).
- **Error:** Returns a negative errno value cast to `u64` (i.e. `-errno as u64`).
- **Unknown syscall number:** Returns `u64::MAX`.

A per-process `errno` value is maintained and set by syscalls on error.
Userspace **must** check the return value — a negative value (or `u64::MAX`)
indicates an error, and `errno` contains the specific error code.

### Error codes

| Code | Name            | Meaning                              |
|------|-----------------|--------------------------------------|
| 2    | `ENOENT`        | No such file or directory            |
| 9    | `EBADF`         | Bad file descriptor                  |
| 12   | `ENOMEM`        | Out of memory                        |
| 13   | `EACCES`        | Permission denied                    |
| 14   | `EFAULT`        | Bad address                          |
| 19   | `ENODEV`        | No such device                      |
| 20   | `ENOTDIR`       | Not a directory                      |
| 22   | `EINVAL`        | Invalid argument                     |
| 23   | `ENFILE`        | File table overflow                  |
| 24   | `EMFILE`        | Too many open files                  |
| 25   | `ENOTTY`        | Inappropriate ioctl for device       |
| 29   | `ESPIPE`        | Illegal seek                         |
| 30   | `EROFS`         | Read-only filesystem                 |
| 36   | `ENAMETOOLONG`  | File name too long                   |
| 38   | `ENOSYS`        | Function not implemented             |

Each syscall defines its own specific return convention (see
[Syscall Reference](#syscall-reference)).

---

## Executable Format

VIBIX supports:

- **Flat binaries** — output of `nasm -f bin`. The first byte of the binary is
  the first instruction executed (equivalent to `_start`). Used for the built-in
  init program.
- **ELF64 executables** — standard `ET_EXEC` files. The loader parses `PT_LOAD`
  segments, allocates physical pages, maps them at the requested virtual addresses
  with appropriate permissions, and zero-fills BSS.

Programs are loaded from the embedded initramfs at `/sbin/init` (PID 1) or via
the `exec` syscall from any VFS path.

### Building for VIBIX

**ELF64 executable:**
```makefile
my_program: my_program.asm
    nasm -f elf64 $< -o my_program.o
    ld -o my_program my_program.o
```

ELF executables can be loaded at runtime via `exec()` from any path the VFS
can resolve (currently initramfs and devfs).

---

## Process Model

VIBIX supports **multi-process preemptive multitasking** with:

- **Up to 64 processes** in a fixed-size process table.
- **Round-robin scheduling** — the PIT timer (100 Hz) triggers context switches
  via `scheduler_tick()`.
- **Process states:** Ready, Running, Blocked (waiting on child), Zombie
  (exited, waiting for parent to reap).
- **Idle process (PID 2):** HLT loop that runs when no other process is ready.
- **Per-process kernel stacks** (4 KiB each) switched atomically during context
  switch. TSS.RSP0 is updated on every switch for correct syscall entry.
- **`fork()`** creates a child with a copy of the parent's kernel stack and
  shared file descriptors (OpenFileTable refcounted).
- **`exec()`** replaces the current process image from a VFS path (ELF64 or
  flat binary), resets brk, and closes fds >= 3.
- **`waitpid()`** blocks the parent until a child exits, then reaps the zombie.
- **`exit()`** marks the process as Zombie, wakes the parent if blocked, and
  triggers a reschedule.

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
on demand, zeroed to prevent kernel-data leaks. The `mmap` syscall (#11) can
also allocate anonymous memory outside the brk area.

### Kernel address space

The kernel resides at `0x200000` (2 MiB) and is mapped with supervisor-only
pages.  Userspace code cannot access kernel memory.

---

## Syscall Reference

### 0 — exit

```c
void exit(int code);
```

**Description:** Terminates the calling process. The process is marked Zombie
with the given exit code. If the parent is blocked in `waitpid()` waiting for
this process, the parent is woken and made Ready. The scheduler then picks the
next runnable process.

**Arguments:**
- `rdi` = `code`: Exit status (convention: `0` = success).

**Return:** Never returns (reschedules).

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
`fd`. Dispatches through the VFS layer: fd → OpenFileTable → vnode → vnode write op.

For `/dev/ttyS0` (fd 0/1/2 by default), writes go to the serial console
(COM1 at 115200 8N1).

**Arguments:**
- `rdi` = `fd`: File descriptor.
- `rsi` = `buf`: Pointer to buffer in user address space.
- `rdx` = `count`: Number of bytes to write.

**Return:** Number of bytes written on success, negative errno on error.

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
`buf`. Dispatches through VFS (same as write).

For `/dev/ttyS0` (fd 0/1/2 by default), reads from the PS/2 keyboard circular
buffer (non-blocking, may return 0). The keyboard driver decodes scan code set
1 into ASCII, buffers keystrokes in a 256-byte ring buffer.

**Arguments:**
- `rdi` = `fd`: File descriptor.
- `rsi` = `buf`: Pointer to buffer in user address space.
- `rdx` = `count`: Maximum number of bytes to read.

**Return:** Number of bytes read, or negative errno on error.

---

### 3 — getpid

```c
pid_t getpid(void);
```

**Description:** Returns the process ID of the calling process.

**Arguments:** None.

**Return:** Process ID (PID 1 = init, PID 2 = idle, 3+ for forked children).

**Example:**
```asm
mov rax, 3
syscall          ; ⟶ rax = pid
```

### 4 — brk

```c
int brk(void *addr);
```

**Description:** Changes the program break (end of the data segment / start of
the heap). Per-process — each process has its own brk.

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

### 5 — nanosleep

```c
int nanosleep(unsigned int sec, unsigned int nsec);
```

**Description:** Busy-waits for the specified duration using PIT ticks
(100 Hz tick rate = 10 ms resolution). If `nsec >= 1_000_000_000`, returns -1.

**Arguments:**
- `rdi` = `sec`: Seconds.
- `rsi` = `nsec`: Nanoseconds (must be < 1_000_000_000).

**Return:** 0 on success, `u64::MAX` on error.

---

### 6 — uname

```c
int uname(struct utsname *buf);
```

**Description:** Fills a fixed-size structure with system identification
strings. Structure layout (each field is 65 bytes):

```c
struct utsname {
    char sysname[65];    // "VIBIX"
    char nodename[65];   // "vibix"
    char release[65];    // "0.1.0"
    char version[65];    // "#1 PREEMPT Tue Jun 23 2026"
    char machine[65];    // "x86_64"
    char domainname[65]; // "VIBIX"
};
```

**Arguments:**
- `rdi` = `buf`: Pointer to a 390-byte buffer in user space.

**Return:** 0 on success.

---

### 7 — reboot

```c
int reboot(int magic, int magic2, int cmd);
```

**Description:** Reboots or powers off the system via QEMU ISA port 0x604.
Requires `magic == 0xfee1dead` and `magic2 == 0x28121969`.

Supported commands:
- `0xcdef0123` (LINUX_REBOOT_CMD_RESTART): Reboot via 0x604 port.
- `0x4321fedc` (LINUX_REBOOT_CMD_POWER_OFF): Power off via 0x604 port.

**Arguments:**
- `rdi` = `magic`: Must be `0xfee1dead`.
- `rsi` = `magic2`: Must be `0x28121969`.
- `rdx` = `cmd`: Command constant.

**Return:** 0 on success, `u64::MAX` on invalid magic/command.

---

### 8 — fork

```c
pid_t fork(void);
```

**Description:** Creates a new process (child) that is a copy of the calling
process (parent). The child gets a new PID, a new kernel stack (copied from
parent), and shares the parent's open file descriptors (OFT refcounts
incremented). The child's return value from `fork()` is 0; the parent receives
the child's PID.

**Limitations (MVP):**
- No copy-on-write — the child shares the parent's address space (single page
  table).
- No process group or session support.
- File descriptors are shared (same offset — correct Unix semantics).

**Arguments:** None.

**Return:** Child PID (>= 3) to parent, 0 to child, or `u64::MAX` on error
(table full / ENOMEM).

---

### 9 — exec

```c
int exec(const char *path, char *const argv[], char *const envp[]);
```

**Description:** Replaces the current process image with a new program loaded
from the VFS path. Supports both ELF64 executables and flat binaries.

**Behavior:**
1. Copies the path string from user space.
2. Resolves the path through VFS (`vfs_resolve`).
3. Loads the binary — ELF64 loader or flat binary loader.
4. Closes all file descriptors >= 3.
5. Resets brk to `BRK_START`.
6. Updates the in-flight syscall return state to redirect to the new entry point.

**Arguments:**
- `rdi` = `path`: Pointer to path string (e.g. "/sbin/init").
- `rsi` = `argv`: Array of argument strings (currently ignored).
- `rdx` = `envp`: Array of environment strings (currently ignored).

**Return:** 0 on success, negative errno on error (process image is not replaced
on error).

---

### 10 — waitpid

```c
pid_t waitpid(pid_t pid, int *wstatus, int flags);
```

**Description:** Waits for a child process to change state. Currently only
supports waiting for termination.

**Behavior:**
- If a matching child is already Zombie, reaps it immediately (returns child PID).
- If the child is still running, blocks the parent (`ProcessState::Blocked`)
  and reschedules. The parent is woken when the child exits.
- If `pid == -1`, waits for any child.
- If no matching child exists, returns -1 (ECHILD).

**Arguments:**
- `rdi` = `pid`: Specific child PID, or -1 for any child.
- `rsi` = `wstatus`: Pointer to store exit code (can be 0 to ignore).
- `rdx` = `flags`: Wait options (currently ignored; blocking is the only mode).

**Return:** Child PID on success, or `u64::MAX` on error / no matching child.

---

### 11 — mmap

```c
void *mmap(void *addr, size_t length, int prot, int flags, int fd, off_t offset);
```

**Description:** Memory-mapped files and anonymous mappings. MVP supports
only `MAP_ANONYMOUS` (with or without `MAP_FIXED`). `fd` and `offset` are
ignored (must be -1 and 0 for MAP_ANONYMOUS).

**Arguments:**
- `rdi` = `addr`: Hint address (ignored without MAP_FIXED).
- `rsi` = `length`: Size of mapping (rounded up to page boundary).
- `rdx` = `prot`: Protection flags (PROT_READ=1, PROT_WRITE=2, PROT_EXEC=4).
- `r8`  = `flags`: MAP_SHARED=0x01, MAP_PRIVATE=0x02, MAP_FIXED=0x10, MAP_ANONYMOUS=0x20.

**Return:** Mapped address on success, or `MAP_FAILED` (`u64::MAX`) on error.
Sets `errno` to `EINVAL` (invalid args/unaligned fixed address), `ENOSYS`
(non-anonymous), or `ENOMEM` (out of memory).

---

### 12 — open

```c
int open(const char *path, int flags, ...);
```

**Description:** Opens a file or device by VFS path. Resolves the path,
allocates an OpenFileTable entry, and returns the lowest available file
descriptor.

**Arguments:**
- `rdi` = `path`: Path string (currently absolute paths only).
- `rsi` = `flags`: Open flags (O_RDONLY=0, O_WRONLY=1, O_RDWR=2, O_CREAT=0x200, O_TRUNC=0x400).
- `rdx` = `mode`: Permission bits (ignored for MVP).

**Return:** File descriptor (>= 0) on success, or negative errno on error:
- `-ENOENT`: Path not found.
- `-ENFILE`: Open file table full.
- `-EMFILE`: Process fd table full.

---

### 13 — close

```c
int close(int fd);
```

**Description:** Closes a file descriptor. The underlying OpenFileTable
entry's refcount is decremented; the entry is freed when refcount reaches 0.

**Arguments:**
- `rdi` = `fd`: File descriptor to close.

**Return:** 0 on success, or `-EBADF` if fd is invalid.

---

### 14 — read (VFS)

```c
ssize_t read(int fd, void *buf, size_t count);
```

**Description:** VFS-dispatched read (same as syscall 2 but through the VFS
layer). Reads up to `count` bytes from the file descriptor's vnode read op.

**Arguments:**
- `rdi` = `fd`: File descriptor.
- `rsi` = `buf`: User-space buffer.
- `rdx` = `count`: Maximum bytes to read.

**Return:** Number of bytes read, or negative errno on error.

---

### 15 — write (VFS)

```c
ssize_t write(int fd, const void *buf, size_t count);
```

**Description:** VFS-dispatched write (same as syscall 1 but through VFS).
Writes up to `count` bytes through the vnode write op.

**Arguments:**
- `rdi` = `fd`: File descriptor.
- `rsi` = `buf`: User-space buffer.
- `rdx` = `count`: Number of bytes to write.

**Return:** Number of bytes written, or negative errno on error.

---

### 16 — lseek

```c
off_t lseek(int fd, off_t offset, int whence);
```

**Description:** Repositions the file offset for the open file description
associated with `fd`. Supports `SEEK_SET` (0), `SEEK_CUR` (1), and `SEEK_END`
(2). For character devices that don't support seeking, returns `-ESPIPE`.

**Arguments:**
- `rdi` = `fd`: File descriptor.
- `rsi` = `offset`: Offset in bytes.
- `rdx` = `whence`: 0=SEEK_SET, 1=SEEK_CUR, 2=SEEK_END.

**Return:** New offset on success, or negative errno on error.

---

### 17 — getdents

```c
int getdents(int fd, struct dirent *dirp, int count);
```

**Description:** Reads directory entries from a directory file descriptor.
Each call returns up to `count` entries.

**Arguments:**
- `rdi` = `fd`: Directory file descriptor.
- `rsi` = `dirp`: Pointer to dirent buffer (see `open_file::Dirent`).
- `rdx` = `count`: Maximum number of entries to read.

**Return:** Number of entries written on success, or negative errno on error.

---

### 18 — dup

```c
int dup(int oldfd);
```

**Description:** Duplicates a file descriptor. The new fd is the lowest
available number. The underlying OpenFileTable entry's refcount is incremented.

**Arguments:**
- `rdi` = `oldfd`: Existing file descriptor.

**Return:** New fd on success, or `-EBADF` / `-EMFILE` on error.

---

### 19 — dup2

```c
int dup2(int oldfd, int newfd);
```

**Description:** Duplicates a file descriptor to a specific fd number.
Follows Linux conventions:
- If `oldfd == newfd`, returns `newfd` (no-op).
- If `newfd` is already open, closes it first.

**Arguments:**
- `rdi` = `oldfd`: Existing file descriptor.
- `rsi` = `newfd`: Target fd number.

**Return:** `newfd` on success, or `-EBADF` on error.

---

### 20 — pipe

```c
int pipe(int pipefd[2]);
```

**Description:** Creates a unidirectional data pipe. `pipefd[0]` is the read
end, `pipefd[1]` is the write end. Data written to the write end can be read
from the read end.

Uses a shared 4 KiB ring buffer (kmalloc'd). The two ends have separate vnodes
with separate VnodeOps tables. The PipeBuf has a refcount of 2 — the buffer is
freed when both ends are closed.

**Arguments:**
- `rdi` = `pipefd`: Pointer to a user-space array of two `i32`s.

**Return:** 0 on success, or negative errno on error.

---

## Change Log

| Date       | Version | Changes                                                |
|------------|---------|--------------------------------------------------------|
| 2026-06-25 | 2.0     | Full rewrite: 19 syscalls, multi-process, VFS, pipes  |
| 2026-06-22 | 1.1     | Added brk (#4), keyboard-backed read, errno, ELF loader |
| 2026-06-20 | 1.0     | Initial release (exit, write, read, getpid)            |
