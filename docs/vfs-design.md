# VIBIX VFS Architecture

> **⚠️ Status: IMPLEMENTED** — This design document describes the VFS layer
> that has been fully implemented. See the source at `kernel_rust/src/vfs/`
> for the actual implementation. Reference this doc for architecture decisions
> and rationale.

## Current State Summary

| Component | Current | Target |
|-----------|---------|--------|
| File descriptors | fd 0 = keyboard+serial, fd 1 = serial, all others EBADF | VFS-aware: per-process fd table → open file table → vnode ops |
| sys_read(2) | Hardcoded fd==0 check, reads keyboard ring buffer + serial fallback | Dispatches through vnode read op |
| sys_write(1) | Hardcoded fd==1 check, writes to COM1 serial port | Dispatches through vnode write op |
| File data | None — no filesystem at all | initramfs with embedded ustar + devfs device nodes |
| Path resolution | None — no path operations | VFS path walk with mount point detection |
| Mounts | None | Root mount (/) + devfs mount (/dev) |
| exec | Reloads userspace binary from hardcoded kernel `.data` blob | Loads ELF from initramfs via `tar_find()` |

---

## 1. Data Structures

### 1.1 Vnode

The vnode is the central abstraction. Every file, directory, or device in the VFS is represented by one.

```rust
/// Vnode type flags (stored in v_type, matching mode bits).
pub const V_DIR:  u16 = 0o040000;  // directory
pub const V_FILE: u16 = 0o100000;  // regular file
pub const V_CHR:  u16 = 0o020000;  // character device

/// Maximum number of entries in the global open file table.
pub const OFT_SIZE: usize = 64;

/// Maximum file descriptors per process.
pub const MAX_FDS: usize = 16;

/// Maximum mount entries.
pub const MAX_MOUNTS: usize = 4;

/// Vnode operation function pointer types.
///
/// Every operation receives the vnode `*mut Vnode` as first argument.
/// Return values: 0 = success, negative = -errno.
pub type VnOpen   = unsafe fn(*mut Vnode, flags: u32, mode: u32) -> i32;
pub type VnClose  = unsafe fn(*mut Vnode) -> i32;
pub type VnRead   = unsafe fn(*mut Vnode, buf: *mut u8, len: usize, offset: &mut u64) -> isize;
pub type VnWrite  = unsafe fn(*mut Vnode, buf: *const u8, len: usize, offset: &mut u64) -> isize;
pub type VnLseek  = unsafe fn(*mut Vnode, offset: i64, whence: u32) -> i64;
pub type VnReaddir = unsafe fn(*mut Vnode, dirent: *mut Dirent, index: u32) -> i32;
pub type VnIoctl  = unsafe fn(*mut Vnode, request: u64, arg: u64) -> i32;

/// Vnode operations table — function pointers for each operation.
///
/// `None` means the operation is not supported (returns -ENOSYS).
#[repr(C)]
pub struct VnodeOps {
    pub open:    Option<VnOpen>,
    pub close:   Option<VnClose>,
    pub read:    Option<VnRead>,
    pub write:   Option<VnWrite>,
    pub lseek:   Option<VnLseek>,
    pub readdir: Option<VnReaddir>,
    pub ioctl:   Option<VnIoctl>,
}

/// Filesystem type identifier.
#[repr(u32)]
pub enum FsType {
    RootFS   = 0,  // synthetic in-memory root
    DevFS    = 1,  // device filesystem
    InitramFS = 2, // initramfs backed by ustar archive
}

/// A virtual inode — one per file, directory, or device.
#[repr(C)]
pub struct Vnode {
    /// Vnode operations table (shared across all vnodes of the same type).
    pub ops: &'static VnodeOps,
    /// file type + permissions (e.g. V_CHR | 0o644)
    pub mode: u16,
    /// inode number (unique within the filesystem)
    pub ino: u64,
    /// file size in bytes (0 for devices)
    pub size: u64,
    /// filesystem type
    pub fs_type: FsType,
    /// fs-specific private data (e.g. dev-minor, initramfs data pointer)
    pub data: *mut (),
    /// If non-null, this directory vnode is a mount point and `mount`
    /// points to the root vnode of the mounted filesystem.
    pub mount: Option<&'static mut Vnode>,
}
```

### 1.2 Open File Description (Global Open File Table)

Separates file descriptors from the underlying file state — the standard Unix design. An `OpenFile` holds the vnode, the current file offset, and flags. When a process calls `open()`, a new `OpenFile` is allocated in the global table, and the process's fd table gets an index into it.

```rust
/// File status flags (open mode).
pub const O_RDONLY: u32 = 0;
pub const O_WRONLY: u32 = 1;
pub const O_RDWR:   u32 = 2;
pub const O_CREAT:  u32 = 0x0200;
pub const O_TRUNC:  u32 = 0x0400;

#[repr(C)]
pub struct OpenFile {
    /// The underlying vnode (immutable while open).
    pub vnode: Option<&'static mut Vnode>,
    /// Current file offset (for lseek / read / write).
    pub offset: u64,
    /// Open flags (O_RDONLY, O_WRONLY, O_RDWR, etc.).
    pub flags: u32,
    /// Open mode (permissions, from open(..., mode)).
    pub mode: u32,
    /// Reference count: number of fd slots pointing here.
    /// Also incremented by dup/dup2.
    pub refcount: u32,
}

/// Global open file table — entries are allocated on open, freed on last close.
#[repr(C)]
pub struct OpenFileTable {
    pub entries: [Option<OpenFile>; OFT_SIZE],
}
```

### 1.3 Per-Process File Descriptor Table

Each process has a small fixed-size array. Entries are indices into the global `OpenFileTable`, or -1 for unused.

```rust
/// File descriptor entry — per-process.
pub type FdEntry = i32;  // index into oft, or -1 = unused

/// Per-process fd table — embedded in Process struct.
pub struct FdTable {
    pub fds: [FdEntry; MAX_FDS],  // -1 = free slot
    pub cwd: Option<&'static mut Vnode>,  // current working directory
}
```

### 1.4 Mount Table

```rust
#[repr(C)]
pub struct MountEntry {
    /// Mount point path (e.g. "/" or "/dev").
    pub path: &'static str,
    /// Root vnode of the mounted filesystem.
    pub root: Option<&'static mut Vnode>,
    /// Filesystem type.
    pub fs_type: FsType,
    /// Mount flags (MS_RDONLY, etc.).
    pub flags: u32,
}

#[repr(C)]
pub struct MountTable {
    pub entries: [Option<MountEntry>; MAX_MOUNTS],
    pub count: usize,
}
```

### 1.5 devfs Private Data

```rust
/// Identity of a devfs device node — enough to dispatch to the correct handler.
#[repr(u32)]
pub enum DevId {
    Null   = 0,
    Zero   = 1,
    TtyS0  = 2,
}
```

### 1.6 initramfs Directory Entry

Built at boot by parsing the embedded ustar archive.

```rust
#[repr(C)]
pub struct InitramfsEntry {
    /// Nul-terminated path (e.g. "sbin/init\0")
    pub name: *const u8,
    /// Pointer to file data in the archive (valid for the kernel's lifetime).
    pub data: *const u8,
    /// File size in bytes.
    pub size: u64,
    /// Entry type: '0' = file, '5' = directory.
    pub entry_type: u8,
}

/// initramfs state — populated once at boot.
#[repr(C)]
pub struct Initramfs {
    pub entries: *mut InitramfsEntry,   // kmalloc'd array
    pub count: usize,
}
```

---

## 2. Design Decisions

### Q1: Vnode abstraction

**Decision:** Vnode with function-pointer ops table.

Each vnode carries a `&'static VnodeOps` reference — a table of `Option<Vn*>` function pointers. The ops table is a `static` constant shared by all vnodes of a given type (e.g. one ops table for all devfs char devices, one for all initramfs regular files).

Operations:
| Op | Required? | Used by |
|----|-----------|---------|
| open | Yes | sys_open — allows device-specific init |
| close | Yes | sys_close — device teardown |
| read | Yes | sys_read |
| write | Yes | sys_write |
| lseek | Yes | sys_lseek — for regular files / devnull / devzero |
| readdir | Directories only | getdents |
| ioctl | Devices only | sys_ioctl — device control (e.g. baud rate) |
| mmap | No (stubbed) | Stubbed for MVP |

**Note:** `mmap` is deliberately omitted for MVP. It would be needed for file-backed memory mapping, but we have `brk` for heap and no demand-paging yet.

### Q2: File descriptor table

**Decision:** Per-process, embedded in the `Process` struct.

- Location: in `Process`, alongside `brk`, `errno`, etc.
- Size: 16 entries for MVP (MAX_FDS = 16).
- Each entry: `i32` index into the global open file table, or -1 for unused.
- Fd 0/1/2 are pre-populated at process creation: all point to `/dev/ttyS0` open file descriptions.
- On fork: fd table is shallow-copied (fd entries and their refcounts in the OFT are incremented).

Rationale: Per-process is simpler and more correct than global. Having the fd table in the Process struct means it's automatically handled by fork/exec — no separate allocation.

### Q3: Open file descriptions

**Decision:** Separate global OpenFileTable (OFT), not direct vnode pointers from fd table.

This is the classic Unix separation:
- FD table (per-process): small array of indices.
- Open file table (global): `[Option<OpenFile>; OFT_SIZE]` — each entry has vnode, offset, flags, refcount.
- A `dup()` or `fork()` creates a new FD pointing to the same OFT entry (incrementing refcount).
- `close()` decrements refcount; the OFT entry is freed only when refcount reaches 0.

Why: Without this separation, `dup()` and `fork()` semantics are wrong — both parent and child should share the same file offset, and `dup()` should create a new fd pointing to the same offset. With vnode-only, these semantics are impossible.

**Global singleton** (same pattern as PROCESS_TABLE):
```rust
static mut OPEN_FILE_TABLE: OpenFileTable = OpenFileTable::new();
```

Access pattern: interrupts disabled, same as `process_mut()`.

### Q4: Mount table

**Decision:** Small fixed-size array (MAX_MOUNTS = 4). Global static.

- Slot 0: `/` (rootfs)
- Slot 1: `/dev` (devfs)
- Slots 2-3: reserved for future use (e.g. `/proc`, `/tmp`)

Mount point lookup: during VFS path resolution, after resolving each path component, the VFS checks whether the resulting vnode has `vnode.mount != None`. If so, it follows the mount to the filesystem root.

Mount entries are stored by path string for `mount()` syscall lookup:
```rust
pub fn vfs_mount(path: &str, root: &'static mut Vnode, fs_type: FsType) -> i32;
```

### Q5: Root filesystem

**Decision:** rootfs is a minimal in-memory directory tree. initramfs is NOT a separate mount — its contents are parsed at boot and used to populate the rootfs directory tree. devfs IS a separate mount at `/dev`.

Boot-time layout:
```
/                          ← rootfs (in-memory)
├── dev/                   ← mount point for devfs
│   ├── null               ← synthesized on lookup
│   ├── zero               ← synthesized on lookup
│   └── ttyS0              ← synthesized on lookup
├── sbin/
│   └── init               ← backed by initramfs data
├── etc/
│   └── ...                ← backed by initramfs data
└── ...                    ← backed by initramfs data
```

How this works:
1. At boot, the embedded ustar archive is parsed. A `Vec`/array of `InitramfsEntry` is built from kmalloc.
2. `vfs_init()` builds a simple tree of vnodes for directories (just enough to allow path traversal).
3. File vnodes for initramfs files point to the data in the archive via `vnode.data`.
4. devfs is registered as a mount on `/dev`.
5. `exec` loads ELF binaries by finding a vnode through path resolution, reading its data through `vnode.ops.read()`.

Alternative considered: making devfs the root and initramfs a mount. Rejected because it complicates the common case (root filesystem should hold regular files).

### Q6: devfs ops dispatch

**Decision:** A single VnodeOps struct for all devfs character devices. Each vnode carries a `DevId` in its `data` pointer. The shared ops functions dispatch on `DevId`:

```rust
static DEVFS_OPS: VnodeOps = VnodeOps {
    open:   Some(devfs_open),
    close:  Some(devfs_close),
    read:   Some(devfs_read),
    write:  Some(devfs_write),
    lseek:  Some(devfs_lseek),
    readdir: Some(devfs_readdir),
    ioctl:  Some(devfs_ioctl),
};
```

Dispatch example for read:
```rust
unsafe fn devfs_read(vnode: *mut Vnode, buf: *mut u8, len: usize, offset: &mut u64) -> isize {
    match *(vnode).data as u32 {
        DEV_NULL => 0,
        DEV_ZERO => { core::ptr::write_bytes(buf, 0, len); len as isize }
        DEV_TTYS0 => serial_read(buf, len),
        _ => -ENODEV,
    }
}
```

Each device:
| Device  | read                        | write               | lseek       |
|---------|-----------------------------|---------------------|-------------|
| /dev/null | returns 0                  | discards, returns len | OK          |
| /dev/zero | fills buf with 0           | discards, returns len | OK          |
| /dev/ttyS0 | reads from serial COM1    | writes to serial COM1 | ESPIPE      |

When a devfs vnode is looked up, a fresh vnode is allocated (or returned from a static set) with the appropriate DevId. We use a static array of 3 dev vnodes pre-initialized:

```rust
static mut DEVFS_VNODES: [Vnode; 3] = [ /* null, zero, ttyS0 */ ];
```

devfs `lookup()` returns a pointer to the matching static vnode.

### Q7: initramfs utility format

**Decision:** Parse the embedded ustar archive at boot into a flat array of `InitramfsEntry` structs. Use linear search for lookups (O(n) per lookup — acceptable for MVP with <100 entries).

At boot:
```rust
/// Parse the embedded ustar archive and build entry table.
/// Called once during VFS init.
pub fn initramfs_init(archive: &[u8]) -> Initramfs {
    // Walk 512-byte header blocks.
    // For each valid header:
    //   - Extract name, size, type
    //   - Allocate InitramfsEntry from kmalloc
    //   - Point data into the archive slice
    // Stop at two consecutive zero blocks.
}
```

Lookup:
```rust
/// Find an initramfs entry by path. Returns (data_ptr, size) or None.
pub fn initramfs_find(path: &[u8]) -> Option<(*const u8, u64)>;
```

The vnode `.read` for initramfs files simply copies from the data pointer:
```rust
unsafe fn initramfs_read(vnode: *mut Vnode, buf: *mut u8, len: usize, offset: &mut u64) -> isize {
    let data = (*vnode).data as *const u8;
    let size = (*vnode).size;
    let remain = size.saturating_sub(*offset);
    let copy_len = (len as u64).min(remain) as usize;
    core::ptr::copy_nonoverlapping(data.add(*offset as usize), buf, copy_len);
    *offset += copy_len as u64;
    copy_len as isize
}
```

For writable tmpfs-like roots in the future, we would use a different backing. For MVP, initramfs is read-only.

### Q8: Syscall interface

**Decision:** Keep existing syscall numbers (sys_read = 2, sys_write = 1). Add new syscalls at unused numbers.

| Num | Name | Current | VFS Target |
|-----|------|---------|------------|
| 0 | exit | unchanged | unchanged |
| 1 | write | hardcoded fd 1 → serial | VFS dispatch |
| 2 | read | hardcoded fd 0 → keyboard+serial | VFS dispatch |
| 3 | getpid | unchanged | unchanged |
| 4 | brk | unchanged | unchanged |
| 5 | nanosleep | unchanged | unchanged |
| 6 | uname | unchanged | unchanged |
| 7 | reboot | unchanged | unchanged |
| 8 | fork | unchanged | unchanged (fd table shallow-copied) |
| 9 | exec | loads from hardcoded blob | loads ELF from initramfs via path |
| 10 | waitpid | unchanged | unchanged |
| **11** | **mmap** | — | **NEW** anonymous memory mapping |
| **12** | **open** | — | **NEW** VFS path→fd |
| **13** | **close** | — | **NEW** fd→void |
| **14** | **read (VFS)** | — | **NEW** VFS dispatch read |
| **15** | **write (VFS)** | — | **NEW** VFS dispatch write |
| **16** | **lseek** | — | **NEW** fd→offset |
| **17** | **getdents** | — | **NEW** read directory entries |
| **18** | **dup** | — | **NEW** fd duplication |
| **19** | **dup2** | — | **NEW** fd duplication to specific number |
| **20** | **pipe** | — | **NEW** inter-process pipe |

**Key changes to existing sys_read/sys_write:**

```rust
fn sys_read(fd: u64, buf: u64, len: u64, _: u64) -> u64 {
    let pid = current_pid();
    let proc = process_mut(pid);
    let oft_index = proc.fd_table.fds[fd as usize];
    if oft_index < 0 { return u64::MAX; }  // EBADF

    let oft = open_file_table();
    if let Some(ref mut of) = oft.entries[oft_index as usize] {
        if let Some(ref mut vn) = of.vnode {
            let ops = vn.ops;
            if let Some(read_fn) = ops.read {
                let result = unsafe { read_fn(vn as *mut _, buf as *mut u8, len as usize, &mut of.offset) };
                if result < 0 { u64::MAX } else { result as u64 }
            } else {
                u64::MAX  // ENOSYS
            }
        } else { u64::MAX }  // EBADF
    } else { u64::MAX }  // EBADF
}
```

**Backward compatibility:** fd 0/1/2 are pre-populated in init's fd table to point to `/dev/ttyS0`. Existing userspace that calls `read(0, …)` and `write(1, …)` continues to work unchanged.

### Q9: Integration with process.rs

**Decision:** Add fd table to `Process` struct. Add VFS init call early in boot.

Changes to `Process` struct:
```rust
#[repr(C)]
#[derive(Debug, Clone)]
pub struct Process {
    // ... existing fields ...
    pub brk: u64,
    pub errno: i64,
    pub name: [u8; 32],

    // NEW: VFS fields
    pub fd_table: FdTable,
}

#[repr(C)]
pub struct FdTable {
    pub fds: [i32; MAX_FDS],  // -1 = unused; else index into open file table
}
```

On fork: shallow-copy the entire `FdTable` (the `i32` array is copied by value). Increment refcount on each `OpenFile` entry that's now shared.

On exec:
- Close all FDs with `FD_CLOEXEC` semantics (optional for MVP; for simple MVP, close all fds except 0/1/2).
- Keep fd 0/1/2 pointing to `/dev/ttyS0`.

On process exit:
- Walk fd table, call `close` on each open file, decrement refcounts.

**Root and cwd vnodes:** `FdTable` already includes `cwd`. Root vnode is implicit (the first mount entry). For MVP we don't store per-process root (`chroot` can be added later).

### Q10: What to design vs what to stub

| Feature | Status | Notes |
|---------|--------|-------|
| Vnode + ops dispatch | **Implement** | Core abstraction |
| Per-process fd table | **Implement** | In Process struct |
| OpenFileTable (global) | **Implement** | OFT_SIZE=64 |
| sys_open | **Implement** | Basic path resolution |
| sys_close | **Implement** | Standard fd close |
| sys_read (VFS) | **Implement** | Replace hardcoded |
| sys_write (VFS) | **Implement** | Replace hardcoded |
| sys_lseek | **Implement** | For regular files |
| sys_ioctl | **Stub** | Returns ENOSYS or ESPIPE |
| sys_mount | **Stub** | Returns ENOSYS (mounts set up at boot only) |
| sys_getdents | **Implement** | For directory listing |
| devfs | **Implement** | /dev/null, /dev/zero, /dev/ttyS0 |
| initramfs parse | **Implement** | ustar parser |
| initramfs vnode read | **Implement** | Read from archive memory |
| Path resolution | **Implement** | Walk path components with mount traversal |
| Mount table | **Implement** | Static, populated at boot |
| exec from VFS | **Implement** | Load ELF from initramfs vnode |
| fork + fd table | **Implement** | Shallow copy + refcount |
| dup/dup2 | **Implement** | ✅ Done (syscalls 18-19) |
| pipe | **Implement** | ✅ Done (syscall 20) |
| mmap (file-backed) | **Stub** | Future |
| stat/fstat | **Stub** | Future (but trivial: fill from vnode) |
| chdir | **Stub** | Would need cwd support |

---

## 3. Filesystem Implementations

### 3.1 rootfs (Minimal In-Memory)

rootfs provides the directory skeleton. It is NOT a full tmpfs — it contains just enough directory vnodes to allow path traversal to initramfs-backed files and the `/dev` mount point.

```
struct RootFs {
    // Pre-allocated directory vnodes for known paths
    root: Vnode,      // "/"
    dev: Vnode,       // "/dev" — mounted by devfs
    sbin: Vnode,      // "/sbin"
    etc: Vnode,       // "/etc"
    bin: Vnode,       // "/bin"
}
```

rootfs directory entries are laid out statically (or built at boot from the initramfs entry list). Lookup in rootfs directories consults the initramfs entry list to see what children exist.

### 3.2 devfs

devfs is a simple synthetic filesystem. It has no persistent storage.

- **Lookup:** If the name matches "null", "zero", or "ttyS0", return a static vnode with the corresponding `DevId` in `.data`.
- **Readdir:** Returns the three device entries: `.`, `..`, `null`, `zero`, `ttyS0`.
- **Everything else:** Dispatched through the shared `DEVFS_OPS` table.

Static vnodes:
```rust
/// Pre-allocated devfs vnodes — live for the kernel's lifetime.
static mut DEV_NULL_VNODE:  Vnode = Vnode { ops: &DEVFS_OPS, mode: V_CHR | 0o666, ino: 1, size: 0, fs_type: FsType::DevFS, data: &raw const DEV_NULL_ID as *mut (), mount: None };
static mut DEV_ZERO_VNODE:  Vnode = Vnode { ops: &DEVFS_OPS, mode: V_CHR | 0o666, ino: 2, size: 0, fs_type: FsType::DevFS, data: &raw const DEV_ZERO_ID as *mut (), mount: None };
static mut DEV_TTYS0_VNODE: Vnode = Vnode { ops: &DEVFS_OPS, mode: V_CHR | 0o666, ino: 3, size: 0, fs_type: FsType::DevFS, data: &raw const DEV_TTYS0_ID as *mut (), mount: None };
```

### 3.3 initramfs

The embedded ustar archive is a `&[u8]` obtained via `include_bytes!`.

At boot, `initramfs_init()`:
1. Walks 512-byte header blocks
2. For each valid header, allocates an `InitramfsEntry` from kmalloc
3. Stores a pointer into the archive data (not a copy)
4. Counts entries

Path resolution for initramfs-backed files:
- `initramfs_find("sbin/init")` → linear scan of entries
- Returns `(data_ptr, size)` → stored in the vnode's `.data` and `.size`

Reads directly from the kernel image's `.data` section (the embedded bytes). No copying.

---

## 4. Syscall Handlers

### 4.1 sys_open

```rust
fn sys_open(path: u64, flags: u64, mode: u64, _: u64) -> u64 {
    let path_str = unsafe { cstr_from_user(path) };
    let pid = current_pid();
    let proc = process_mut(pid);

    // 1. Resolve path to vnode (walk mount table, traverse components)
    let vnode = match vfs_resolve(path_str) {
        Ok(vn) => vn,
        Err(e) => { proc.errno = e as i64; return u64::MAX; }
    };

    // 2. Call vnode.open (device-specific init)
    if let Some(open_fn) = vnode.ops.open {
        let ret = unsafe { open_fn(vnode, flags as u32, mode as u32) };
        if ret < 0 { proc.errno = -ret as i64; return u64::MAX; }
    }

    // 3. Allocate OpenFile + fd slot
    let oft_idx = oft_alloc(vnode, flags as u32, mode as u32);
    let fd = fd_alloc(proc, oft_idx);
    fd as u64
}
```

### 4.2 sys_close

```rust
fn sys_close(fd: u64, _: u64, _: u64, _: u64) -> u64 {
    if fd >= MAX_FDS as u64 { return u64::MAX; }  // EBADF
    let pid = current_pid();
    let proc = process_mut(pid);
    let oft_idx = proc.fd_table.fds[fd as usize];
    if oft_idx < 0 { return u64::MAX; }  // EBADF

    proc.fd_table.fds[fd as usize] = -1;
    oft_decref(oft_idx as usize);
    0
}
```

### 4.3 sys_read (VFS dispatch, replaces current)

```rust
fn sys_read(fd: u64, buf: u64, len: u64, _: u64) -> u64 {
    if fd >= MAX_FDS as u64 { return u64::MAX; }
    let pid = current_pid();
    let proc = process_mut(pid);
    let oft_idx = proc.fd_table.fds[fd as usize];
    if oft_idx < 0 { return u64::MAX; }

    let oft = unsafe { &mut OPEN_FILE_TABLE };
    if let Some(ref mut of) = oft.entries[oft_idx as usize] {
        let vnode = of.vnode.as_mut().unwrap();
        if let Some(read_fn) = vnode.ops.read {
            let result = unsafe { read_fn(&raw mut *vnode, buf as *mut u8, len as usize, &mut of.offset) };
            if result < 0 { proc.errno = -result as i64; return u64::MAX; }
            result as u64
        } else { u64::MAX }  // ENOSYS
    } else { u64::MAX }  // EBADF
}
```

### 4.4 sys_write (VFS dispatch, replaces current)

```rust
fn sys_write(fd: u64, buf: u64, len: u64, _: u64) -> u64 {
    if fd >= MAX_FDS as u64 { return u64::MAX; }
    let pid = current_pid();
    let proc = process_mut(pid);
    let oft_idx = proc.fd_table.fds[fd as usize];
    if oft_idx < 0 { return u64::MAX; }

    let oft = unsafe { &mut OPEN_FILE_TABLE };
    if let Some(ref mut of) = oft.entries[oft_idx as usize] {
        let vnode = of.vnode.as_mut().unwrap();
        if let Some(write_fn) = vnode.ops.write {
            let result = unsafe { write_fn(&raw mut *vnode, buf as *const u8, len as usize, &mut of.offset) };
            if result < 0 { proc.errno = -result as i64; return u64::MAX; }
            result as u64
        } else { u64::MAX }
    } else { u64::MAX }
}
```

### 4.5 sys_lseek

```rust
fn sys_lseek(fd: u64, offset: u64, whence: u64, _: u64) -> u64 {
    if fd >= MAX_FDS as u64 { return u64::MAX; }
    let pid = current_pid();
    let proc = process_mut(pid);
    let oft_idx = proc.fd_table.fds[fd as usize];
    if oft_idx < 0 { return u64::MAX; }

    let oft = unsafe { &mut OPEN_FILE_TABLE };
    if let Some(ref mut of) = oft.entries[oft_idx as usize] {
        let vnode = of.vnode.as_mut().unwrap();
        if let Some(lseek_fn) = vnode.ops.lseek {
            let result = unsafe { lseek_fn(&raw mut *vnode, offset as i64, whence as u32) };
            if result < 0 { proc.errno = -result as i64; return u64::MAX; }
            result as u64
        } else { u64::MAX }
    } else { u64::MAX }
}
```

Notable: lseek returns the new offset. For character devices that don't support seeking, the vnode op returns `-ESPIPE`.

### 4.6 sys_getdents

```rust
fn sys_getdents(fd: u64, dirent: u64, count: u64, _: u64) -> u64 {
    // Resolve fd → vnode
    // If vnode is not a directory, return -ENOTDIR
    // Call vnode.readdir(index=n) repeatedly to fill user buffer
    // Returns bytes written to dirent buffer
}
```

### 4.7 sys_exec (modified to load from VFS)

```rust
fn sys_exec(path: u64, argv: u64, envp: u64, _: u64) -> u64 {
    let path_str = unsafe { cstr_from_user(path) };

    // 1. Resolve path through VFS to find the ELF vnode
    let vnode = vfs_resolve(path_str);
    if vnode.is_err() { return u64::MAX; }
    let vnode = vnode.unwrap();

    // 2. Read the entire file into a temporary buffer
    //    (For MVP, read into a stack-allocated or kmalloc'd buffer,
    //     up to some max file size — e.g. 256 KB)
    let mut buf = [0u8; 256 * 1024];
    let mut offset = 0u64;
    let read_fn = vnode.ops.read.unwrap();
    let bytes_read = unsafe { read_fn(vnode, buf.as_mut_ptr(), buf.len(), &mut offset) };
    if bytes_read <= 0 { return u64::MAX; }

    // 3. Load ELF from buffer
    let pmm = pmm::global_pmm();
    match elf::load(&buf[..bytes_read as usize], pmm) {
        Ok(entry) => {
            let pid = current_pid();
            let proc = process_mut(pid);
            proc.entry = entry;
            proc.user_rsp = USER_STACK_ADDR + 0x1000;
            proc.brk = BRK_START;
            proc.errno = 0;

            // Redirect syscall return
            unsafe {
                syscall_state.rip = entry;
                syscall_state.rsp = USER_STACK_ADDR + 0x1000;
                syscall_state.rflags = 0x202;
            }
            0
        }
        Err(_) => u64::MAX,
    }
}
```

---

## 5. VFS Path Resolution

```rust
/// Resolve a path string to a vnode.
///
/// Handles:
///   - Absolute paths (starting with '/')
///   - Mount point crossing (vnode.mount != None)
///   - "." and ".." components
///
/// Returns a pointer to the resolved vnode, or an errno.
pub fn vfs_resolve(path: &[u8]) -> Result<&'static mut Vnode, i32> {
    let root = vfs_get_root();  // mount table entry 0

    if path.is_empty() || path[0] != b'/' {
        return Err(-ENOENT);  // relative paths not supported yet (needs cwd)
    }

    let mut current = root;
    // Skip leading '/'
    let remain = &path[1..];
    if remain.is_empty() {
        return Ok(current);  // path == "/"
    }

    // Split path by '/' and resolve each component
    for component in remain.split(|&b| b == b'/') {
        if component.is_empty() || component == b"." {
            continue;
        }
        if component == b".." {
            // For MVP, parent navigation is limited — return EINVAL
            return Err(-EINVAL);
        }

        // Look up component in current directory
        // For rootfs directories, scan InitramfsEntry list for children
        // For devfs directories, match against known names
        let child = vfs_lookup(current, component)?;

        // Check if child is a mount point
        if let Some(mount_root) = child.mount.as_mut() {
            current = mount_root;
        } else {
            current = child;
        }
    }

    Ok(current)
}
```

### 5.1 vfs_lookup helper

```rust
/// Look up a single path component in a directory vnode.
fn vfs_lookup(dir: &mut Vnode, name: &[u8]) -> Result<&'static mut Vnode, i32> {
    match dir.fs_type {
        FsType::RootFS => rootfs_lookup(dir, name),
        FsType::DevFS => devfs_lookup(dir, name),
        FsType::InitramFS => initramfs_lookup(dir, name),
    }
}
```

---

## 6. Boot Sequence Changes

Current boot flow (`lib.rs`):
```
serial → pmm → kmm → paging → fb → interrupts → pit → keyboard → gdt/syscall → enable_ints → spawn_init → scheduler
```

New boot flow:
```
serial → pmm → kmm → paging → fb → interrupts → pit → keyboard → gdt/syscall
  → VFS INIT ← NEW
  → initramfs parse ← NEW
  → devfs mount ← NEW
  → enable_ints → spawn_init (with fd 0/1/2 set up) → scheduler
```

Detailed VFS init sequence:
```rust
pub fn vfs_init() {
    // 1. Parse embedded ustar archive → InitramfsEntry array
    let archive: &[u8] = include_bytes!("../../initramfs.tar");  // or similar
    initramfs::init(archive);

    // 2. Build rootfs directory vnodes
    rootfs::init();

    // 3. Register devfs mount at "/dev"
    let dev_root = devfs::init();
    mount_table_add("/", rootfs_root_vnode(), FsType::RootFS);
    mount_table_add("/dev", dev_root, FsType::DevFS);

    // 4. Register syscalls 11-16
    syscall::register(11, sys_open);
    syscall::register(12, sys_close);
    syscall::register(13, sys_lseek);
    syscall::register(14, sys_ioctl);
    syscall::register(15, sys_mount);
    syscall::register(16, sys_getdents);
}
```

Changes to `process::spawn_init()`:
```rust
pub fn spawn_init(pmm: &mut PmmAllocator) -> u64 {
    load_init_binary(pmm);

    // ... existing process creation ...

    let mut proc = Process {
        // ... existing fields ...
        fd_table: FdTable {
            fds: [-1; MAX_FDS],
        },
    };

    // Open fd 0, 1, 2 for init → all point to /dev/ttyS0
    let tty_vnode = devfs_get_vnode(DevId::TtyS0);
    let oft_idx = oft_alloc(tty_vnode, O_RDWR, 0o666);
    proc.fd_table.fds[0] = oft_idx as i32;
    proc.fd_table.fds[1] = oft_idx as i32;  // same OpenFile for stdout
    proc.fd_table.fds[2] = oft_idx as i32;  // same OpenFile for stderr
    // refcount = 3

    // ... rest of spawn_init ...
}
```

---

## 7. C Runtime Support

### 7.1 errno values

```rust
pub const ENOENT:  i32 = 2;
pub const EBADF:   i32 = 9;
pub const ENOMEM:  i32 = 12;
pub const EACCES:  i32 = 13;
pub const ENODEV:  i32 = 19;
pub const EINVAL:  i32 = 22;
pub const ENOSYS:  i32 = 38;
pub const ENOTDIR: i32 = 20;
pub const EROFS:   i32 = 30;
pub const ESPIPE:  i32 = 29;
```

---

## 8. Lifetime / Safety Considerations

### 8.1 Static vnodes (devfs)

devfs vnodes are `static mut` — they live for the kernel's entire lifetime. This is safe because:
- They are only accessed with interrupts disabled (same as PROCESS_TABLE).
- The ops tables are `&'static` constants, never mutated.
- The `data` field is set once at init and never changed.

### 8.2 Heap-allocated initramfs entries

Initramfs entries are allocated from kmalloc at boot and **never freed**. This is acceptable for MVP:
- The initramfs is part of the kernel image — it's always needed.
- If we wanted to free/reclaim, we'd need true reference counting on vnodes.
- Memory cost: each `InitramfsEntry` is ~24 bytes (name ptr + data ptr + size + type). With 100 entries, ~2.4 KB.

### 8.3 OpenFile table refcounts

The `OpenFile` entries have a `refcount` field. When processes fork:
- Parent's fd table is shallow-copied to child.
- For each shared FD, `oft[fd].refcount += 1`.
- `close()` calls `oft_decref()`; when refcount hits 0, the entry is freed.

### 8.4 Vnode reuse after close

Vnodes themselves are NOT reference-counted in MVP (they live forever — devfs vnodes are static, initramfs vnodes leak). This is correct because:
- devfs vnodes are static and never freed.
- initramfs vnodes are only created at boot and represent immutable files.

The only thing freed is the `OpenFile` entry in the global OFT. The vnode pointer inside it remains valid.

### 8.5 Rust borrow checker bypass

The VFS code uses `*mut Vnode` and `&'static mut Vnode` extensively, bypassing Rust's borrow checker. This is the same pattern used throughout the kernel (`process_mut()` returns `&'static mut Process`). The safety invariant is:

> **All VFS data structures are accessed only with interrupts disabled**, ensuring single-threaded access within the kernel. The entry points are syscall handlers (which run with IF=0 in Ring 0) and boot init.

### 8.6 User-supplied path strings

`open()` receives a user-space pointer. We need a helper to copy the string from user space into a kernel buffer (with length limit):

```rust
/// Copy a nul-terminated string from user space into a kernel buffer.
/// Returns the valid path length (excluding nul) or -EFAULT.
/// Safety: user pointer must be valid for up to PATH_MAX bytes.
const PATH_MAX: usize = 4096;

unsafe fn cstr_from_user(ptr: u64) -> Result<[u8; PATH_MAX], i32> {
    let mut buf = [0u8; PATH_MAX];
    for i in 0..PATH_MAX {
        let byte = *(ptr as *const u8).add(i);
        if byte == 0 { return Ok(buf); }
        buf[i] = byte;
    }
    Err(-EFAULT)  // No nul terminator within PATH_MAX
}
```

---

## 9. initramfs Boot Integration

### 9.1 ustar archive embedding

The initramfs archive is a flat binary embedded in the kernel image. The build process:

```makefile
# In Makefile (userspace/Makefile or kernel_rust/Makefile snippet):
initramfs.tar: $(shell find initramfs/ -type f)
	tar cf $@ -C initramfs .

# In kernel_rust/build.rs or via include_bytes!:
# // Unchanged — the Rust code uses:
# include_bytes!("../../initramfs.tar")
```

The userspace initramfs directory structure:
```
userspace/initramfs/
├── sbin/
│   └── init          # ELF binary (first process)
├── bin/
│   └── sh            # shell (placeholder)
└── etc/
    └── passwd        # (optional)
```

### 9.2 initramfs parser

```rust
/// ustar header block — 512 bytes.
#[repr(C)]
struct UstarHeader {
    name:     [u8; 100],   // file name
    mode:     [u8; 8],     // file mode (octal ASCII)
    uid:      [u8; 8],
    gid:      [u8; 8],
    size:     [u8; 12],    // file size (octal ASCII)
    mtime:    [u8; 12],
    chksum:   [u8; 8],
    typeflag: u8,           // '0'=file, '5'=dir
    linkname: [u8; 100],
    magic:    [u8; 6],      // "ustar\0"
    version:  [u8; 2],
    uname:    [u8; 32],
    gname:    [u8; 32],
    devmajor: [u8; 8],
    devminor: [u8; 8],
    prefix:   [u8; 155],    // filename prefix
    padding:  [u8; 12],
}
```

Parser loop:
```rust
pub fn init(archive: &'static [u8]) {
    let mut offset = 0usize;
    let mut entries = Vec::new();  // using kmalloc-allocated array

    while offset + 512 <= archive.len() {
        let hdr = &archive[offset..offset + 512];

        // Check for end marker (two zero blocks)
        if hdr.iter().all(|&b| b == 0) { break; }

        // Parse header
        let name = parse_name(hdr);
        let size = parse_octal(&hdr[124..136]);
        let typeflag = hdr[156];

        if typeflag == b'0' || typeflag == b'5' {
            let data_offset = offset + 512;
            entries.push(InitramfsEntry {
                name: kmalloc_name_copy(name),  // copy to kernel heap
                data: &archive[data_offset] as *const u8,
                size,
                entry_type: typeflag,
            });
        }

        // Advance to next header (data rounded to 512)
        let data_blocks = ((size + 511) / 512) * 512;
        offset += 512 + data_blocks;
    }

    // Store entries in a static/global
    initramfs_set_entries(entries);
}
```

---

## 10. Implementation Phases

### Phase 1: VFS Core + devfs ✅ DONE

Files to create:
| File | Contents |
|------|----------|
| `kernel_rust/src/vfs/mod.rs` | Top-level: vnode struct, VnodeOps, path resolution, errno definitions |
| `kernel_rust/src/vfs/open_file.rs` | OpenFile, OpenFileTable, oft_alloc, oft_decref |
| `kernel_rust/src/vfs/mount.rs` | MountTable, mount_add, mount_find |
| `kernel_rust/src/vfs/devfs.rs` | devfs vnodes, ops dispatch, device implementations |
| `kernel_rust/src/vfs/initramfs.rs` | ustar parser, entry lookup, vnode builder |

Changes to existing files:
| File | Changes |
|------|---------|
| `kernel_rust/src/process.rs` | Add `fd_table: FdTable` to Process; update fork to shallow-copy fds and refcount; update spawn_init to set up fd 0/1/2 |
| `kernel_rust/src/syscall.rs` | Relpace hardcoded read/write with VFS dispatch; add sys_open, sys_close, sys_lseek, sys_ioctl, sys_mount, sys_getdents |
| `kernel_rust/src/lib.rs` | Add `vfs::init()` after syscall init; add `initramfs.tar` to link |
| `kernel_rust/src/elf.rs` | No change (already reads from `&[u8]` buffer — receives data from VFS) |
| `Makefile` | Add target for building `initramfs.tar` |

### Phase 2: Initramfs-backed exec + remaining syscalls ✅ DONE

- Modify `sys_exec` to resolve path through VFS, read ELF data from initramfs vnode
- Add `sys_getdents` for directory listing
- Add `chdir` support via `FdTable.cwd`

### Phase 3: Polish ✅ PARTIAL

- ✅ `dup`/`dup2` syscalls — done
- ✅ `pipe` syscall — done (syscall 20)
- ❌ O_CLOEXEC handling on exec — TODO
- ❌ File permission checking (mode bits) — TODO
- ❌ `stat`/`fstat` syscalls — see [phase3c.md](phase3c.md)
- ❌ Refcount vnodes for dynamic filesystems — TODO

---

## 11. File-by-File Change Plan

### New Files

| File | Description |
|------|-------------|
| `kernel_rust/src/vfs/mod.rs` | Vnode, VnodeOps, vfs_resolve, vfs_init, errno constants |
| `kernel_rust/src/vfs/open_file.rs` | OpenFile, OpenFileTable, oft_alloc, oft_decref |
| `kernel_rust/src/vfs/mount.rs` | MountTable, mount_add, mount_find |
| `kernel_rust/src/vfs/devfs.rs` | DevId, devfs_init, devfs_lookup, static vnodes, DEVFS_OPS |
| `kernel_rust/src/vfs/initramfs.rs` | InitramfsEntry, initramfs_init, initramfs_find, tar parsing |
| `kernel_rust/src/vfs/rootfs.rs` | RootFS directory vnodes, rootfs_lookup |

### Modified Files

| File | Changes |
|------|---------|
| `kernel_rust/src/process.rs` | Add `fd_table: FdTable` to Process, `pub struct FdTable`, update fork/spawn_init/exit |
| `kernel_rust/src/syscall.rs` | Rewrite read/write to VFS dispatch, add open/close/lseek/ioctl/mount/getdents handlers, register new syscalls in `init()` |
| `kernel_rust/src/lib.rs` | Add `mod vfs`, call `vfs::init()` in boot sequence |
| `Makefile` | Add initramfs.tar build target, add `vfs/*.rs` to cargo watch |

### Unchanged

| File | Reason |
|------|--------|
| `kernel_rust/src/paging.rs` | No filesystem dependencies |
| `kernel_rust/src/pmm.rs` | No changes needed |
| `kernel_rust/src/kmm.rs` | Already provides heap for vnode allocation |
| `kernel_rust/src/elf.rs` | Already reads from `&[u8]` — no change |
| `kernel_rust/src/serial.rs` | No change (used through devfs) |
| `kernel_rust/src/keyboard.rs` | No change (used through devfs) |
| `kernel_rust/src/gdt.rs` | No change |

---

## 12. Edge Cases

### File descriptor exhaustion
- Per-process: 16 FDs. `open()` returns -1 (EMFILE) when all slots used.
- Global open file table: 64 entries. Returns -1 (ENFILE) when exhausted.

### Path too long
- PATH_MAX = 4096 bytes. If no nul terminator found within that, return -ENAMETOOLONG (or -EFAULT).

### Open file table refcount overflow
- 32-bit refcount field. Max 65535 theoretical. In practice: 16 FDs × 64 processes = 1024 max refs. No overflow risk.

### Fork with shared OFT entries
- After fork, parent and child share the same `OpenFile` entries. `close()` in one does not close for the other (refcount mechanism). The offset is shared — `lseek` in one affects the other. This is correct Unix semantics.

### Exec with open FDs
- MVP: Keep fd 0/1/2 open. Close all other FDs. Later: respect O_CLOEXEC flag.
- If fd 0/1/2 point to `/dev/ttyS0` and that vnode is static, no special teardown needed.

### Multiple opens of /dev/ttyS0
- Each `open()` gets a fresh `OpenFile` entry with its own offset (which is irrelevant for serial — always 0). The underlying vnode is shared (static). This is correct.

### initramfs archive corruption
- The archive is embedded in the kernel at compile time, not read from disk. If it's corrupt, it's a build error (tar would fail). No runtime corruption expected.

### Process exit with open FDs
- Walk fd table: for each open fd, call `oft_decref()` which may free the OpenFile entry.
- If `vnode.ops.close` exists, call it before decrementing.

---

## 13. Summary

The design provides a minimal but correct VFS layer:
- **Separation of concerns:** vnode (what), vnode ops (how), open file descriptions (where we are), fd table (who has it open) — all separate.
- **devfs:** Character devices via static vnodes with shared ops table.
- **initramfs:** Read-only backing for boot files via ustar archive.
- **Rootfs:** Minimal directory skeleton over initramfs data.
- **Path resolution:** Component-by-component walk with mount point crossing.
- **Compatibility:** Existing syscall numbers unchanged; fd 0/1/2 still work.
- **Safety:** Same interrupt-disabled pattern as rest of kernel.

Total new code: ~600-800 lines across 5 new files, ~100 lines of changes to existing files.
