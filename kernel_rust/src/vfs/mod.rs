//==============================================================================
// vfs/mod.rs — Virtual File System core
//
// Provides the Vnode abstraction, VnodeOps function-pointer table,
// mount management, path resolution, and VFS initialisation.
//
// All VFS mutable statics use the same pattern as PROCESS_TABLE:
// accessed only with interrupts disabled.
//==============================================================================

pub mod open_file;
pub mod mount;
pub mod devfs;
pub mod chardev;
pub mod pipe;
pub mod initramfs;
pub mod rootfs;
pub mod tty;

//==============================================================================
// Vnode type flags (Unix-style mode bits)
//==============================================================================

pub const V_DIR:  u16 = 0o040000;
pub const V_FILE: u16 = 0o100000;
pub const V_CHR:  u16 = 0o020000;

//==============================================================================
// Open flags
//==============================================================================

pub const O_RDONLY: u32 = 0;
pub const O_WRONLY: u32 = 1;
pub const O_RDWR:   u32 = 2;
pub const O_CREAT:  u32 = 0x0200;
pub const O_TRUNC:  u32 = 0x0400;

//==============================================================================
// Limits
//==============================================================================

pub const MAX_FDS: usize = 16;
pub const OFT_SIZE: usize = 64;
pub const MAX_MOUNTS: usize = 4;
pub const PATH_MAX: usize = 256;
pub const VFS_MAX_COMPONENTS: usize = 16;

//==============================================================================
// Errno values
//==============================================================================

pub const ENOENT:  i32 = 2;
pub const EBADF:   i32 = 9;
pub const ENOMEM:  i32 = 12;
pub const EACCES:  i32 = 13;
pub const EFAULT:  i32 = 14;
pub const ENODEV:  i32 = 19;
pub const ENOTDIR: i32 = 20;
pub const EINVAL:  i32 = 22;
pub const ENOSYS:  i32 = 38;
pub const EMFILE:  i32 = 24;
pub const ENFILE:  i32 = 23;
pub const ESPIPE:  i32 = 29;
pub const EROFS:   i32 = 30;
pub const ENAMETOOLONG: i32 = 36;
pub const ENOTTY:  i32 = 25;

//==============================================================================
// Vnode operation function-pointer types
//==============================================================================

pub type VnOpen   = unsafe fn(*mut Vnode, flags: u32, mode: u32) -> i32;
pub type VnClose  = unsafe fn(*mut Vnode) -> i32;
pub type VnRead   = unsafe fn(*mut Vnode, buf: *mut u8, len: usize, offset: &mut u64) -> isize;
pub type VnWrite  = unsafe fn(*mut Vnode, buf: *const u8, len: usize, offset: &mut u64) -> isize;
pub type VnLseek  = unsafe fn(*mut Vnode, offset: i64, whence: u32) -> i64;
pub type VnReaddir = unsafe fn(*mut Vnode, dirent: *mut crate::vfs::open_file::Dirent, index: u32) -> i32;
pub type VnIoctl  = unsafe fn(*mut Vnode, request: u64, arg: u64) -> i32;

//==============================================================================
// VnodeOps — table of function pointers for a filesystem
//==============================================================================

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

//==============================================================================
// FsType — filesystem type identifier
//==============================================================================

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsType {
    RootFS    = 0,
    DevFS     = 1,
    InitramFS = 2,
}

//==============================================================================
// Vnode — in-memory representation of a file or directory
//==============================================================================

#[repr(C)]
pub struct Vnode {
    pub ops: *const VnodeOps,
    pub mode: u16,
    pub ino: u64,
    pub size: u64,
    pub fs_type: FsType,
    /// Filesystem-private data (e.g. DevId, initramfs data pointer, path prefix).
    pub data: *mut (),
    /// If this vnode is a mount point, points to the mounted root vnode.
    pub mount: Option<&'static mut Vnode>,
}

//==============================================================================
// Stat — x86_64 Linux struct stat layout (144 bytes)
//==============================================================================

/// x86_64 Linux struct stat layout.
/// Must match what userspace `fstat/stat` expects.
#[repr(C)]
pub struct Stat {
    pub st_dev:     u64,      // 0
    pub st_ino:     u64,      // 8
    pub st_nlink:   u64,      // 16
    pub st_mode:    u32,      // 24
    pub st_uid:     u32,      // 28
    pub st_gid:     u32,      // 32
    __pad0:         u32,      // 36
    pub st_rdev:    u64,      // 40
    pub st_size:    i64,      // 48
    pub st_blksize: i64,      // 56
    pub st_blocks:  i64,      // 64
    pub st_atim_sec:  i64,    // 72
    pub st_atim_nsec: i64,    // 80
    pub st_mtim_sec:  i64,    // 88
    pub st_mtim_nsec: i64,    // 96
    pub st_ctim_sec:  i64,    // 104
    pub st_ctim_nsec: i64,    // 112
    __unused:       [i64; 3], // 120
} // total: 144 bytes

//==============================================================================
// stat_from_vnode — fill a Stat struct from a Vnode
//==============================================================================

pub fn stat_from_vnode(vn: &Vnode, st: &mut Stat) {
    st.st_dev = 0;
    st.st_ino = vn.ino;
    st.st_nlink = 1;
    st.st_mode = vn.mode as u32;
    st.st_uid = 0;
    st.st_gid = 0;
    st.__pad0 = 0;
    st.st_rdev = if vn.mode & V_CHR != 0 {
        0x0101
    } else {
        0
    };
    st.st_size = vn.size as i64;
    st.st_blksize = 512;
    st.st_blocks = ((vn.size + 511) / 512) as i64;
    st.st_atim_sec = 0;
    st.st_atim_nsec = 0;
    st.st_mtim_sec = 0;
    st.st_mtim_nsec = 0;
    st.st_ctim_sec = 0;
    st.st_ctim_nsec = 0;
    st.__unused = [0; 3];
}

//==============================================================================
// FdTable — per-process file descriptor table
//==============================================================================

/// Per-process file descriptor table.
/// Maps fd numbers (0..MAX_FDS) to indices into the Open File Table.
/// -1 means the fd is unused.
#[derive(Debug, Clone, Copy)]
pub struct FdTable {
    pub fds: [i32; MAX_FDS],
}

impl FdTable {
    pub const fn new() -> Self {
        FdTable { fds: [-1; MAX_FDS] }
    }
}

// Helper: cstr_from_user — copy a null-terminated string from user space
//==============================================================================

/// Copy a null-terminated string from user space into a fixed-size buffer.
///
/// `ptr` is the user-space address.  At most `max_len` bytes are copied
/// (including the implicit NUL terminator).  Returns a byte slice of the
/// copied string (without the NUL terminator) or an error errno.
///
/// # Safety
///
/// Caller must ensure interrupts are disabled and the buffer is local.
pub unsafe fn cstr_from_user(ptr: *const u8, max_len: usize) -> Result<[u8; PATH_MAX], i32> {
    if ptr.is_null() {
        return Err(EFAULT);
    }
    let cap = PATH_MAX.min(max_len);
    let mut buf = [0u8; PATH_MAX];
    for i in 0..cap {
        let byte = *ptr.add(i);
        if byte == 0 {
            buf[i] = 0;
            return Ok(buf);
        }
        buf[i] = byte;
    }
    Err(ENAMETOOLONG)
}

/// Compare a byte-slice name against a NUL-terminated C string.
pub unsafe fn path_eq(name: &[u8], cstr: *const u8) -> bool {
    if name.is_empty() || cstr.is_null() {
        return false;
    }
    let mut i = 0usize;
    while i < name.len() {
        let c = *cstr.add(i);
        if c == 0 {
            return false;
        }
        if name[i] != c {
            return false;
        }
        i += 1;
    }
    *cstr.add(i) == 0
}

//==============================================================================
// vfs_lookup — dispatch to filesystem-specific lookup
//==============================================================================

/// Look up `name` (single path component) within directory `dir`.
///
/// Dispatches based on `dir.fs_type`.
pub fn vfs_lookup(dir: &mut Vnode, name: &[u8]) -> Result<&'static mut Vnode, i32> {
    if name.is_empty() || name == b"." {
        return Err(EINVAL);
    }
    if name == b".." {
        return Err(EINVAL);
    }

    match dir.fs_type {
        FsType::RootFS => {
            unsafe { crate::vfs::rootfs::rootfs_lookup(dir, name) }
        }
        FsType::DevFS => {
            match unsafe { crate::vfs::devfs::devfs_lookup(name) } {
                Some(vn) => Ok(vn),
                None => Err(ENOENT),
            }
        }
        FsType::InitramFS => {
            // InitramFS directories are navigated through rootfs
            Err(ENOENT)
        }
    }
}

//==============================================================================
// vfs_resolve — resolve an absolute path to a vnode
//==============================================================================

/// Resolve an absolute path to a `&'static mut Vnode`.
///
/// Uses a two-pass approach:
///   1. Collect path components, handling `.` and `..` natively.
///   2. Resolve accumulated components from root.
/// This avoids recursive parent-pointer lookups while correctly handling
///   `..`, `.`, `//`, and trailing `/`.
pub fn vfs_resolve(path: &[u8]) -> Result<&'static mut Vnode, i32> {
    let root = crate::vfs::mount::mount_get_root()
        .ok_or(ENODEV)?;

    if path.is_empty() || (path.len() == 1 && path[0] == b'/') {
        return Ok(root);
    }

    // Skip leading '/'
    let mut start = 0usize;
    while start < path.len() && path[start] == b'/' {
        start += 1;
    }
    if start >= path.len() {
        return Ok(root);
    }

    //──────────────────────────────────────────────────────────────
    // Pass 1: collect components, handle `.` and `..`
    //──────────────────────────────────────────────────────────────
    let mut comp_start: [usize; VFS_MAX_COMPONENTS] = [0; VFS_MAX_COMPONENTS];
    let mut comp_end:   [usize; VFS_MAX_COMPONENTS] = [0; VFS_MAX_COMPONENTS];
    let mut ncomp: usize = 0;
    let mut pos = start;

    while pos < path.len() {
        // Skip consecutive slashes
        while pos < path.len() && path[pos] == b'/' {
            pos += 1;
        }
        if pos >= path.len() {
            break;
        }

        let cs = pos;
        while pos < path.len() && path[pos] != b'/' {
            pos += 1;
        }

        if ncomp < VFS_MAX_COMPONENTS {
            if &path[cs..pos] == b".." {
                // Pop the previous component — go up one level
                if ncomp > 0 {
                    ncomp -= 1;
                }
                // Already at root — `..` is a no-op (POSIX)
            } else if &path[cs..pos] != b"." {
                // Track this component
                comp_start[ncomp] = cs;
                comp_end[ncomp]   = pos;
                ncomp += 1;
            }
            // `.` → skip (no-op)
        } else {
            return Err(ENAMETOOLONG);
        }
    }

    //──────────────────────────────────────────────────────────────
    // Pass 2: resolve accumulated components from root
    //──────────────────────────────────────────────────────────────
    let mut current = root;
    for i in 0..ncomp {
        let component = &path[comp_start[i]..comp_end[i]];

        match vfs_lookup(&mut current, component) {
            Ok(next) => {
                // Follow mount points
                if let Some(ref mut mounted) = unsafe { &mut *(&raw mut *next) }.mount {
                    current = mounted;
                } else {
                    current = next;
                }
            }
            Err(e) => {
                // Fallback: try accumulated path against initramfs.
                // The accumulated path has `..` already normalised out.
                let accumulated = &path[start..comp_end[i]];
                if !accumulated.is_empty() {
                    if let Some(vn) = unsafe { crate::vfs::initramfs::initramfs_build_vnode(accumulated) } {
                        return Ok(vn);
                    }
                }
                return Err(e);
            }
        }
    }

    Ok(current)
}

/// Resolve a path relative to a current working directory.
/// If `path` starts with '/', it's treated as absolute and passed directly to `vfs_resolve`.
/// Otherwise, concatenates `cwd + "/" + path` into `out_buf` and resolves the result.
/// `out_buf` must be at least `cwd.len() + 1 + path.len()` bytes.
/// Returns `Ok(vnode)` on success, `Err(i32)` with a negative errno on failure.
pub fn vfs_resolve_relative<'a>(cwd: &[u8], path: &[u8]) -> Result<&'static mut Vnode, i32> {
    // If path starts with '/', it's absolute — resolve directly
    if !path.is_empty() && path[0] == b'/' {
        return vfs_resolve(path);
    }

    // Relative path: build cwd + "/" + path in a temporary buffer on the stack
    let mut buf = [0u8; PATH_MAX];
    let mut pos = 0;
    // Copy cwd (already stripped of NUL)
    while pos < cwd.len() {
        buf[pos] = cwd[pos];
        pos += 1;
    }
    // Ensure separator
    if pos == 0 || buf[pos - 1] != b'/' {
        if pos >= PATH_MAX - 1 { return Err(ENAMETOOLONG); }
        buf[pos] = b'/';
        pos += 1;
    }
    // Copy path (already stripped of NUL)
    for &b in path {
        if pos >= PATH_MAX { return Err(ENAMETOOLONG); }
        buf[pos] = b;
        pos += 1;
    }
    vfs_resolve(&buf[..pos])
}

//==============================================================================
// vfs_init — initialise the VFS layer
//==============================================================================

/// Called once during boot, after PMM and KMM are initialised.
pub unsafe fn vfs_init() {
    // 0. Set embedded initramfs archive
    crate::vfs::initramfs::initramfs_embed();

    // 1. Parse initramfs archive (previously skipped when null)
    crate::vfs::initramfs::initramfs_init();


    // 2. Initialise rootfs directory skeleton
    crate::vfs::rootfs::rootfs_init();

    // 3. Initialise devfs — get devfs root vnode
    let dev_root = crate::vfs::devfs::devfs_init();

    // 4. Set up mount table
    // Slot 0: "/" → rootfs
    crate::vfs::mount::mount_add(
        "/",
        crate::vfs::rootfs::rootfs_get_root(),
        FsType::RootFS,
        0,
    );

    // Slot 1: "/dev" → devfs
    let rc = crate::vfs::mount::mount_add("/dev", dev_root, FsType::DevFS, 0);
    if rc != 0 {
        let mut serial = crate::serial::SerialPort::new();
        serial.writestrs(&["VFS: WARNING — mount /dev failed.\n"]);
    } else {
        // Set the mount field on rootfs /dev vnode so vfs_resolve
        // can follow the mount point when resolving paths.
        if let Some(dev_mnt) = crate::vfs::mount::mount_find(b"/dev") {
            crate::vfs::rootfs::DEV_DIR.mount = Some(dev_mnt);
        }
    }

    // 5. Register VFS syscalls (12-17)
    //    Note: syscall 11 is already mmap, so VFS uses 12-17.
    crate::syscall::register(12, sys_open);
    crate::syscall::register(13, sys_close);
    crate::syscall::register(14, sys_read_vfs);
    crate::syscall::register(15, sys_write_vfs);
    crate::syscall::register(16, sys_lseek);
    crate::syscall::register(17, sys_getdents);

    // 6. Register VIBIX-local VFS syscalls (21-23)
    crate::syscall::register(21, sys_stat);
    crate::syscall::register(22, sys_fstat);
    crate::syscall::register(23, sys_chdir);
}

//==============================================================================
// VFS syscall handlers
//==============================================================================

fn sys_open(path: u64, flags: u64, mode: u64, _arg4: u64) -> u64 {
    unsafe {
        // 1. Copy path string from user space
        let buf = match cstr_from_user(path as *const u8, PATH_MAX) {
            Ok(b) => b,
            Err(e) => return (-e as i64) as u64,
        };

        // 2. Find NUL terminator to get actual path length
        let path_len = buf.iter().position(|&b| b == 0).unwrap_or(PATH_MAX);
        let path_slice = &buf[..path_len];

        // 3. Resolve path to vnode
        let vnode = match vfs_resolve(path_slice) {
            Ok(vn) => vn,
            Err(e) => return (-e as i64) as u64,
        };

        // 4. Call vnode open op if present
        if let Some(open_fn) = (*vnode.ops).open {
            let rc = open_fn(vnode as *mut Vnode, flags as u32, mode as u32);
            if rc != 0 {
                return rc as u64;
            }
        }

        // 5. Allocate OFT entry
        let oft_idx = match open_file::oft_alloc(vnode, flags as u32, mode as u32) {
            Ok(idx) => idx,
            Err(e) => return (-e as i64) as u64,
        };

        // 6. Find first free fd
        let fds = &mut crate::process::process_mut(crate::process::current_pid()).fd_table.fds;
        for fd in 0..MAX_FDS {
            if fds[fd] < 0 {
                fds[fd] = oft_idx as i32;
                return fd as u64;
            }
        }

        // 7. No free fd — cleanup and return EMFILE
        open_file::oft_decref(oft_idx);
        (-EMFILE as i64) as u64
    }
}

fn sys_close(fd: u64, _arg2: u64, _arg3: u64, _arg4: u64) -> u64 {
        if fd >= MAX_FDS as u64 {
            return (-EBADF as i64) as u64;
        }
        let fds = &mut crate::process::process_mut(crate::process::current_pid()).fd_table.fds;
        let oft_idx = fds[fd as usize];
        if oft_idx < 0 {
            return (-EBADF as i64) as u64;
        }
        fds[fd as usize] = -1;
        open_file::oft_decref(oft_idx as usize);
        0
}

fn sys_read_vfs(fd: u64, buf: u64, count: u64, _arg4: u64) -> u64 {
    unsafe {
        if fd >= MAX_FDS as u64 {
            return (-EBADF as i64) as u64;
        }
        let fd_entry = crate::process::process_mut(crate::process::current_pid()).fd_table.fds[fd as usize];
        if fd_entry < 0 {
            return (-EBADF as i64) as u64;
        }
        if buf == 0 || count == 0 {
            return (-EINVAL as i64) as u64;
        }

        let of = match open_file::oft_get(fd_entry as usize) {
            Some(of) => of,
            None => return (-EBADF as i64) as u64,
        };
        let of_ptr = of as *mut open_file::OpenFile;

        let vn = match &mut (*of_ptr).vnode {
            Some(v) => *v as *mut Vnode,
            None => return (-EBADF as i64) as u64,
        };

        let read_fn = match (*(*vn).ops).read {
            Some(f) => f,
            None => return (-EBADF as i64) as u64,
        };

        let nread = read_fn(vn, buf as *mut u8, count as usize, &mut (*of_ptr).offset);
        nread as u64
    }
}

fn sys_write_vfs(fd: u64, buf: u64, count: u64, _arg4: u64) -> u64 {
    unsafe {
        if fd >= MAX_FDS as u64 {
            return (-EBADF as i64) as u64;
        }
        let fd_entry = crate::process::process_mut(crate::process::current_pid()).fd_table.fds[fd as usize];
        if fd_entry < 0 {
            return (-EBADF as i64) as u64;
        }
        if buf == 0 || count == 0 {
            return (-EINVAL as i64) as u64;
        }

        let of = match open_file::oft_get(fd_entry as usize) {
            Some(of) => of,
            None => return (-EBADF as i64) as u64,
        };
        let of_ptr = of as *mut open_file::OpenFile;

        let vn = match &mut (*of_ptr).vnode {
            Some(v) => *v as *mut Vnode,
            None => return (-EBADF as i64) as u64,
        };

        let write_fn = match (*(*vn).ops).write {
            Some(f) => f,
            None => return (-EBADF as i64) as u64,
        };

        let nwritten = write_fn(vn, buf as *const u8, count as usize, &mut (*of_ptr).offset);
        nwritten as u64
    }
}

fn sys_lseek(fd: u64, offset: u64, whence: u64, _arg4: u64) -> u64 {
    unsafe {
        if fd >= MAX_FDS as u64 {
            return (-EBADF as i64) as u64;
        }
        let fd_entry = crate::process::process_mut(crate::process::current_pid()).fd_table.fds[fd as usize];
        if fd_entry < 0 {
            return (-EBADF as i64) as u64;
        }

        let of = match open_file::oft_get(fd_entry as usize) {
            Some(of) => of,
            None => return (-EBADF as i64) as u64,
        };
        let of_ptr = of as *mut open_file::OpenFile;

        let vn = match &mut (*of_ptr).vnode {
            Some(v) => *v as *mut Vnode,
            None => return (-EBADF as i64) as u64,
        };

        // Custom lseek op if present
        if let Some(lseek_fn) = (*(*vn).ops).lseek {
            let new_off = lseek_fn(vn, offset as i64, whence as u32);
            if new_off >= 0 {
                (*of_ptr).offset = new_off as u64;
            }
            return new_off as u64;
        }

        // Fallback: SEEK_SET=0, SEEK_CUR=1, SEEK_END=2
        let new_off: i64 = match whence {
            0 => offset as i64,
            1 => ((*of_ptr).offset as i64).wrapping_add(offset as i64),
            2 => ((*vn).size as i64).wrapping_add(offset as i64),
            _ => return (-EINVAL as i64) as u64,
        };
        if new_off < 0 {
            return (-EINVAL as i64) as u64;
        }
        (*of_ptr).offset = new_off as u64;
        new_off as u64
    }
}

fn sys_getdents(fd: u64, dirent: u64, count: u64, _arg4: u64) -> u64 {
    unsafe {
        if fd >= MAX_FDS as u64 {
            return (-EBADF as i64) as u64;
        }
        let fd_entry = crate::process::process_mut(crate::process::current_pid()).fd_table.fds[fd as usize];
        if fd_entry < 0 {
            return (-EBADF as i64) as u64;
        }
        if dirent == 0 || count == 0 {
            return (-EINVAL as i64) as u64;
        }

        let of = match open_file::oft_get(fd_entry as usize) {
            Some(of) => of,
            None => return (-EBADF as i64) as u64,
        };
        let of_ptr = of as *mut open_file::OpenFile;

        let vn = match &mut (*of_ptr).vnode {
            Some(v) => *v as *mut Vnode,
            None => return (-EBADF as i64) as u64,
        };

        let readdir_fn = match (*(*vn).ops).readdir {
            Some(f) => f,
            None => return (-ENOTTY as i64) as u64,
        };

        let mut entries_written: u64 = 0;
        let mut index: u32 = 0;
        let mut dirent_ptr = dirent as *mut u8;

        while entries_written < count {
            let mut db: open_file::Dirent = core::mem::zeroed();
            let ret = readdir_fn(vn, &mut db as *mut open_file::Dirent, index);
            if ret <= 0 {
                break;
            }
            let reclen = db.d_reclen as usize;
            if reclen == 0 || reclen > core::mem::size_of::<open_file::Dirent>() {
                break;
            }
            core::ptr::copy_nonoverlapping(
                &db as *const open_file::Dirent as *const u8,
                dirent_ptr,
                reclen,
            );
            dirent_ptr = dirent_ptr.add(reclen);
            index = index.wrapping_add(1);
            entries_written += 1;
        }
        entries_written
    }
}

//==============================================================================
// stat(21) — get file status by path
//==============================================================================

fn sys_stat(path: u64, statbuf: u64, _arg3: u64, _arg4: u64) -> u64 {
    unsafe {
        // 1. Copy path from user space
        let buf = match cstr_from_user(path as *const u8, PATH_MAX) {
            Ok(b) => b,
            Err(e) => return (-e as i64) as u64,
        };
        let path_len = buf.iter().position(|&b| b == 0).unwrap_or(PATH_MAX);
        let path_slice = &buf[..path_len];

        // 2. Get current process cwd
        let cwd = crate::process::process_mut(crate::process::current_pid()).cwd;
        // Find NUL terminator in cwd
        let cwd_len = cwd.iter().position(|&b| b == 0).unwrap_or(PATH_MAX);
        let cwd_slice = &cwd[..cwd_len];

        // 3. Resolve path (relative to cwd)
        let vn = match vfs_resolve_relative(cwd_slice, path_slice) {
            Ok(vn) => vn,
            Err(e) => return (-e as i64) as u64,
        };

        // 4. Fill stat struct
        let mut st: Stat = core::mem::zeroed();
        stat_from_vnode(&*vn, &mut st);

        // 5. Copy to user space
        core::ptr::copy_nonoverlapping(
            &st as *const Stat as *const u8,
            statbuf as *mut u8,
            core::mem::size_of::<Stat>(),
        );

        0
    }
}

//==============================================================================
// fstat(22) — get file status by fd
//==============================================================================

fn sys_fstat(fd: u64, statbuf: u64, _arg3: u64, _arg4: u64) -> u64 {
    unsafe {
        if fd >= MAX_FDS as u64 {
            return (-EBADF as i64) as u64;
        }
        let fd_entry = crate::process::process_mut(crate::process::current_pid()).fd_table.fds[fd as usize];
        if fd_entry < 0 {
            return (-EBADF as i64) as u64;
        }
        if statbuf == 0 {
            return (-EFAULT as i64) as u64;
        }

        let of = match open_file::oft_get(fd_entry as usize) {
            Some(of) => of,
            None => return (-EBADF as i64) as u64,
        };
        let of_ptr = of as *mut open_file::OpenFile;

        let vn = match &mut (*of_ptr).vnode {
            Some(v) => *v as *mut Vnode,
            None => return (-EBADF as i64) as u64,
        };

        let mut st: Stat = core::mem::zeroed();
        stat_from_vnode(&*vn, &mut st);

        core::ptr::copy_nonoverlapping(
            &st as *const Stat as *const u8,
            statbuf as *mut u8,
            core::mem::size_of::<Stat>(),
        );

        0
    }
}

//==============================================================================
// chdir(23) — change current working directory
//==============================================================================

fn sys_chdir(path: u64, _arg2: u64, _arg3: u64, _arg4: u64) -> u64 {
    unsafe {
        // 1. Copy path from user space
        let buf = match cstr_from_user(path as *const u8, PATH_MAX) {
            Ok(b) => b,
            Err(e) => return (-e as i64) as u64,
        };
        let path_len = buf.iter().position(|&b| b == 0).unwrap_or(PATH_MAX);
        let path_slice = &buf[..path_len];

        // 2. Get current process cwd
        let cwd = crate::process::process_mut(crate::process::current_pid()).cwd;
        let cwd_len = cwd.iter().position(|&b| b == 0).unwrap_or(PATH_MAX);
        let cwd_slice = &cwd[..cwd_len];

        // 3. Resolve path
        let vn = match vfs_resolve_relative(cwd_slice, path_slice) {
            Ok(vn) => vn,
            Err(e) => return (-e as i64) as u64,
        };

        // 4. Must be a directory
        if (*vn).mode & V_DIR == 0 {
            return (-ENOTDIR as i64) as u64;
        }

        // 5. Build new cwd string
        // If path is absolute, use it directly; otherwise, resolve canonical form
        let is_absolute = !path_slice.is_empty() && path_slice[0] == b'/';
        let new_cwd_buf: [u8; PATH_MAX];
        let new_cwd_len: usize;

        if is_absolute {
            // Use the path directly
            new_cwd_len = path_slice.len().min(PATH_MAX - 1);
            new_cwd_buf = {
                let mut c = [0u8; PATH_MAX];
                let mut i = 0;
                while i < new_cwd_len {
                    c[i] = path_slice[i];
                    i += 1;
                }
                c
            };
        } else {
            // Relative path: concatenate cwd + "/" + path
            let cwd_nul = cwd_len; // length without NUL
            let _total = cwd_nul + 1 + path_slice.len().min(PATH_MAX - cwd_nul - 2);
            new_cwd_buf = {
                let mut c = [0u8; PATH_MAX];
                let mut i = 0;
                while i < cwd_nul {
                    c[i] = cwd_slice[i];
                    i += 1;
                }
                // Ensure trailing slash
                if i > 0 && c[i-1] != b'/' {
                    c[i] = b'/';
                    i += 1;
                }
                let mut j = 0;
                while j < path_slice.len() && i < PATH_MAX - 1 {
                    c[i] = path_slice[j];
                    i += 1;
                    j += 1;
                }
                c
            };
            new_cwd_len = {
                let mut len = 0;
                while len < PATH_MAX && new_cwd_buf[len] != 0 {
                    len += 1;
                }
                len
            };
        }

        // 6. Update process cwd (write into the process's cwd buffer)
        let proc = crate::process::process_mut(crate::process::current_pid());
        let mut i = 0;
        while i < new_cwd_len && i < PATH_MAX - 1 {
            proc.cwd[i] = new_cwd_buf[i];
            i += 1;
        }
        proc.cwd[i] = 0; // NUL-terminate

        0
    }
}
