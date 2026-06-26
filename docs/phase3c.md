# Phase 3c: Shell, Init, and ANSI Terminal

**Status:** Planning  
**Target branch:** `scheduler-phase1` (continue from where Phase 1/2 left off)  
**Previous phases:** Preemptive scheduler + multi-process (Phase 1), fork/exec/waitpid (Phase 2), VFS layer (Phase 2b)

---

## Overview

Phase 3c rounds out the kernel with an interactive user experience:
a working shell, a proper init process, and ANSI terminal support
so the system is usable from the serial console.

The core kernel infra (scheduler, VFS, pipes) is done — this phase is
about **userspace programs** and the **terminal driver** they talk to.

---

## What Exists Now

### Kernel
| Component | Location | Status |
|-----------|----------|--------|
| Preemptive round-robin scheduler | `kernel_rust/src/process.rs` | ✅ Working |
| fork/exec/waitpid syscalls | `kernel_rust/src/syscall.rs` | ✅ Working |
| VFS layer (vnode, OFT, mount, devfs, initramfs, rootfs) | `kernel_rust/src/vfs/` | ✅ Working |
| Pipe syscall | `kernel_rust/src/vfs/pipe.rs`, `syscall.rs:20` | ✅ Implemented |
| dup/dup2 syscalls | `kernel_rust/src/syscall.rs` | ✅ Working |
| /dev/ttyS0 (serial char device) | `kernel_rust/src/vfs/devfs.rs` | ✅ Working |
| PS/2 keyboard driver | `kernel_rust/src/keyboard.rs` | ✅ Working |

### Userspace
| Component | Location | Status |
|-----------|----------|--------|
| Init process (PID 1) — flat binary | `userspace/vibix_blob.asm` (via initramfs) | ✅ Boots |
| Simple demo commands | `userspace/vibix_blob.asm` dispatch table | ✅ Basic |
| Userspace build system | `userspace/Makefile` | ✅ Working |
| initramfs.tar embedded in kernel | `userspace/initramfs.tar` | ✅ |

### What's Missing for an Interactive Shell
1. **ANSI terminal driver** — /dev/tty with line discipline (echo, line buffering, editing)
2. **Shell program** — read commands, fork/exec/wait, PATH resolution
3. **Init with service management** — spawn getty on /dev/ttyS0, reap zombies
4. **Signal support** — Ctrl+C, Ctrl+D handling through the terminal driver
5. **stat/fstat syscalls** — needed for shell features (file existence checks)
6. **chdir** — needed for shell `cd` command

---

## Recommended Implementation Order

### Step 1: ANSI Terminal Driver (`kernel_rust/src/vfs/tty.rs`)

**Goal:** Replace raw /dev/ttyS0 with a proper terminal driver that provides
line discipline (canonical mode: line buffering, local echo, signal generation).

**Reference:** The current `devfs.rs` `devfs_read`/`devfs_write` for TtyS0
directly calls `serial_read()`/`serial_write()` — no buffering, no line editing.

**What to implement:**
1. Create `kernel_rust/src/vfs/tty.rs` with a `Tty` struct:
   - Input ring buffer (1-4 KiB from kmalloc)
   - Line discipline state machine:
     - **Canonical mode (default):** Buffer input until `\n` or `\r`. Echo
       characters as they arrive. Process special characters:
       - `\b` (0x08) / `\x7f` (DEL) → erase previous character
       - `\r` / `\n` → echo `\r\n`, make line available to `read()`
       - `\x03` (Ctrl+C) → generate SIGINT to foreground process group
       - `\x1a` (Ctrl+Z) → generate SIGTSTP (future)
   - `MIN`/`TIME` support for non-canonical mode (future)

2. Wire the Tty into the VFS:
   - Replace the raw `/dev/ttyS0` vnode with one backed by the Tty driver.
   - The Tty wraps the serial hardware: `tty_write` → serial out,
     `tty_read` → line buffer in.
   - IRQ1 (keyboard) feeds scancodes into the Tty input (via existing keyboard
     ring buffer or direct).

3. Tty VnodeOps:
   - `open`: Allocate Tty struct if not already (singleton for now).
   - `close`: No-op (kernel console Tty lives forever).
   - `read`: Return from canonical line buffer. Block if empty (future).
   - `write`: Write to serial console directly.
   - `ioctl`: `TCGETS`/`TCSETS` for termios, `TIOCGWINSZ` for window size.

**Files to modify:**
| File | Change |
|------|--------|
| `kernel_rust/src/vfs/tty.rs` | **NEW** — Tty driver with line discipline |
| `kernel_rust/src/vfs/devfs.rs` | Change TtyS0 vnode to use new Tty driver |
| `kernel_rust/src/vfs/mod.rs` | Add `pub mod tty` |
| `kernel_rust/src/keyboard.rs` | Optional: wire keyboard to Tty directly |

**Verification:**
- Boot kernel, type text on serial console, see it echoed with correct line editing.
- `read(0, buf, 1)` on a Tty fd returns one character at a time (non-canonical).
- `read(0, buf, 256)` on a Tty fd returns until `\n` (canonical).

---

### Step 2: stat/fstat Syscalls (syscall 21-22)

**Goal:** Basic file metadata queries needed by shell and user programs.

**Implementation:**
```rust
fn sys_stat(path: u64, statbuf: u64) -> u64;
fn sys_fstat(fd: u64, statbuf: u64) -> u64;
```

The `stat` structure should match Linux's `struct stat` (at least the fields
that make sense for VIBIX):
```c
struct stat {
    uint64_t st_dev;
    uint64_t st_ino;
    uint32_t st_mode;
    uint32_t st_nlink;
    uint32_t st_uid;
    uint32_t st_gid;
    uint64_t st_rdev;
    uint64_t st_size;
    // ... omit times for MVP
};
```

Data comes from the Vnode: `mode`, `ino`, `size` are already there.
`st_dev` = mount entry index. `st_rdev` = DevId for char devices.

**Files to modify:**
| File | Change |
|------|--------|
| `kernel_rust/src/syscall.rs` | Add `sys_stat`, `sys_fstat`, register as 21-22 |

**Verification:**
- Userspace program calls `stat("/sbin/init", &sb)` → gets correct size.
- `fstat(0, &sb)` → gets correct mode (character device).

---

### Step 3: chdir Syscall (syscall 23)

**Goal:** Allow processes to change their current working directory.

**Implementation:**
- Add `cwd: &'static mut Vnode` (or similar) to `FdTable`.
- `sys_chdir(path)`: resolve path, verify it's a directory, update cwd.
- Update `vfs_resolve()` to support relative paths (prepend cwd).

```rust
fn sys_chdir(path: u64, _: u64, _: u64, _: u64) -> u64 {
    let buf = cstr_from_user(path, PATH_MAX)?;
    // ... resolve relative to cwd or absolute ...
}
```

**Files to modify:**
| File | Change |
|------|--------|
| `kernel_rust/src/vfs/mod.rs` | Add `cwd` handling to `FdTable`, support relative paths in `vfs_resolve` |
| `kernel_rust/src/syscall.rs` | Add `sys_chdir`, register as 23 |

---

### Step 4: Shell Program (`userspace/shell/`)

**Goal:** A minimal shell (similar to `sh` or `dash`) that runs on the serial
console.

**Implementation approach:** Write in assembly (NASM ELF64) or start with
assembly and plan a C compiler port later.

**Features (MVP):**
1. Print prompt (`$ ` or `# `).
2. Read command line with line editing (handled by Tty driver in canonical mode).
3. Parse command and arguments (space-separated, simple).
4. `fork()` child, `exec()` the command by resolving through PATH.
5. `waitpid()` for child to complete.
6. Built-in commands: `cd`, `exit`, `help`.

**Shell flow:**
```asm
; Pseudo-code (assembly)
loop:
    write(1, prompt, 2)
    read(0, cmdline, 256)
    parse cmdline → argv
    if argv[0] == "cd" → chdir(argv[1])
    if argv[0] == "exit" → exit(0)
    pid = fork()
    if pid == 0:
        exec(argv[0], argv, envp)
        exit(1)  ; exec failed
    else:
        waitpid(pid, &status, 0)
    goto loop
```

**PATH resolution:** Try `/bin/<cmd>` and `/sbin/<cmd>`.

**Files to create:**
| File | Description |
|------|-------------|
| `userspace/shell/shell.asm` | Shell program source |
| `userspace/shell/Makefile` | Build as ELF64 |
| `userspace/Makefile` (modify) | Build shell and include in initramfs |

---

### Step 5: Init Process Upgrade

**Goal:** The init process (currently a demo binary) should:
1. Start a shell/getty on the serial console.
2. Reap orphaned zombies (loop with `waitpid(-1, ...)`).
3. Handle system shutdown (signal children, sync, reboot).

**Current init (`userspace/vibix_blob.asm`):**
- Has a command dispatch table with simple built-in commands.
- Could be extended to also spawn a shell, or replaced entirely.

**Recommended approach:** Keep the current init binary as PID 1 but have it
fork/exec `/bin/sh` after basic setup. Modify `vibix_blob.asm` to:
```asm
; After basic setup, spawn shell
fork()
if child:
    exec("/bin/sh", ...)
else:
    ; Parent = init
    loop:
        waitpid(-1, &status, 0)   ; reap zombies
        goto loop
```

Or simpler: make the initramfs `/sbin/init` be a shell script interpreter
or just set up so the shell is the init process itself (PID 1).

---

### Step 6: Forward-Looking (Phase 4 Ideas)

Once Phase 3c is stable, the next frontiers are:

| Feature | Priority | Notes |
|---------|----------|-------|
| **Signals** | High | SIGINT from Ctrl+C, SIGCHLD, signal delivery + handlers |
| **Process groups / sessions** | High | Needed for proper job control |
| **COW fork** | Medium | Memory efficiency for large processes |
| **Per-process page tables** | Medium | Address space isolation |
| **tmpfs** | Medium | Writable filesystem for runtime files |
| **AHCI/SATA driver** | Low | Real storage beyond initramfs |
| **RTC driver** | Low | wall-clock time for timestamps |
| **sbrk/brk improvements** | Low | Unmap shrinking pages |
| **Multicore/SMP** | Very Low | Long-term goal |

---

## Agent Handoff Notes

### Code organization
- Kernel Rust code: `kernel_rust/src/`
  - Syscalls: `kernel_rust/src/syscall.rs` — add new handlers here, register in `init()`
  - VFS: `kernel_rust/src/vfs/` — all filesystem-related code
  - Process: `kernel_rust/src/process.rs` — scheduler, process table, fork/exec/waitpid
  - Build: `kernel_rust/Cargo.toml`, `kernel_rust/kernel64_elf.ld`
- Userspace: `userspace/`
  - Build: `userspace/Makefile` (produces `vibix_blob.bin` and `initramfs.tar`)
  - Init program: `userspace/vibix_blob.asm` (flat binary, dispatch table architecture)
  - Include files: `userspace/vibix_*.inc` (shared constants/macros)
- Top-level build: `Makefile` (builds kernel + emebds initramfs)

### Build & test commands
```bash
make          # Build kernel
make run      # Run in QEMU (serial stdio)
make test     # Boot test (validates boot output)
```

### Adding new syscalls
1. Implement handler function in `kernel_rust/src/syscall.rs`
2. Register in `pub fn init()` with `register(num, handler_fn)`
3. Update `SYSCALL.md` with the ABI
4. Create userspace test program

### Adding userspace programs
1. Create directory under `userspace/` (e.g. `userspace/shell/`)
2. Write program as NASM ELF64 or flat binary
3. Add build rules to `userspace/Makefile`
4. The combined `vibix_blob.bin` includes all programs; `initramfs.tar` is
   built from the `userspace/initramfs/` directory tree.

### Current branch
- All changes are on branch `scheduler-phase1`.
- After Phase 3c, create a new branch and PR to `main`.

### Key gotchas
- **Interrupts disabled in syscall handlers** — all VFS/process accesses must
  happen with IF=0 (guaranteed by syscall entry).
- **No heap allocation in interrupt context** — use kmalloc at boot/init time only.
- **Per-process kernel stacks are 4 KiB** — keep stack usage tight.
- **OFT refcounting** — when duplicating fds (fork, dup, dup2), always call
  `oft_incref()`/`oft_decref()` correctly.
- **Userspace pointers** — always copy strings from user space with
  `cstr_from_user()` before dereferencing.
