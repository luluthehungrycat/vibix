//==============================================================================
// vfs/rootfs.rs — Root filesystem directory skeleton
//
// rootfs is the minimal in-memory directory tree that serves as the
// root of the VFS.  It provides the top-level directories (/, /sbin,
// /bin, /etc, /dev) and consults the initramfs for file entries.
//
// For Phase 1 MVP, initramfs files found during lookup are backed by
// vnodes from the initramfs vnode pool.
//
// Mutable statics follow the PROCESS_TABLE pattern: accessed only
// with interrupts disabled.
//==============================================================================

use core::ptr;
use crate::vfs::*;
use crate::vfs::open_file::Dirent;
use crate::vfs::initramfs;

//==============================================================================
// Ops table for rootfs directories
//==============================================================================

static ROOTFS_DIR_OPS: VnodeOps = VnodeOps {
    open:    Some(rootfs_open as VnOpen),
    close:   Some(rootfs_close as VnClose),
    read:    None,
    write:   None,
    lseek:   None,
    readdir: Some(rootfs_readdir as VnReaddir),
    ioctl:   None,
};

//==============================================================================
// Static directory vnodes
//==============================================================================

static mut ROOT_DIR: Vnode = unsafe { core::mem::zeroed() };
static mut SBIN_DIR: Vnode = unsafe { core::mem::zeroed() };
static mut BIN_DIR:  Vnode = unsafe { core::mem::zeroed() };
static mut ETC_DIR:  Vnode = unsafe { core::mem::zeroed() };
pub static mut DEV_DIR:  Vnode = unsafe { core::mem::zeroed() };

//==============================================================================
// Vnode operation implementations
//==============================================================================

unsafe fn rootfs_open(_vnode: *mut Vnode, _flags: u32, _mode: u32) -> i32 { 0 }
unsafe fn rootfs_close(_vnode: *mut Vnode) -> i32 { 0 }

unsafe fn rootfs_readdir(vnode: *mut Vnode, dirent: *mut Dirent, index: u32) -> i32 {
    if dirent.is_null() {
        return -EFAULT;
    }
    let ino = (*vnode).ino;

    if index == 0 {
        let de = &mut *dirent;
        de.d_ino = ino;
        de.d_off = 0;
        de.d_reclen = core::mem::size_of::<Dirent>() as u16;
        de.d_type = V_DIR as u8;
        let name = b".\0";
        let mut i = 0usize;
        while i < name.len() && i < 256 { de.d_name[i] = name[i]; i += 1; }
        while i < 256 { de.d_name[i] = 0; i += 1; }
        return 1;
    }
    if index == 1 {
        let de = &mut *dirent;
        de.d_ino = 0;
        de.d_off = 0;
        de.d_reclen = core::mem::size_of::<Dirent>() as u16;
        de.d_type = V_DIR as u8;
        let name = b"..\0";
        let mut i = 0usize;
        while i < name.len() && i < 256 { de.d_name[i] = name[i]; i += 1; }
        while i < 256 { de.d_name[i] = 0; i += 1; }
        return 1;
    }

    if ino == 0 {
        let child_idx = (index - 2) as usize;
        let (child_ino, child_name, child_mode) = match child_idx {
            0 => (1, b"sbin\0", V_DIR),
            1 => (2, b"bin\0\0",  V_DIR),
            2 => (3, b"etc\0\0",  V_DIR),
            3 => (4, b"dev\0\0",  V_DIR),
            _ => return 0,
        };
        let de = &mut *dirent;
        de.d_ino = child_ino;
        de.d_off = 0;
        de.d_reclen = core::mem::size_of::<Dirent>() as u16;
        de.d_type = child_mode as u8;
        let mut i = 0usize;
        while i < child_name.len() && i < 256 { de.d_name[i] = child_name[i]; i += 1; }
        while i < 256 { de.d_name[i] = 0; i += 1; }
        return 1;
    }

    0
}

//==============================================================================
// rootfs_lookup
//==============================================================================

pub unsafe fn rootfs_lookup(dir: &mut Vnode, name: &[u8]) -> Result<&'static mut Vnode, i32> {
    if dir.ino == 0 {
        match name {
            b"sbin" => return Ok(&mut SBIN_DIR),
            b"bin"  => return Ok(&mut BIN_DIR),
            b"etc"  => return Ok(&mut ETC_DIR),
            b"dev"  => return Ok(&mut DEV_DIR),
            _ => {}
        }
    }

    let mut full_path_buf = [0u8; PATH_MAX];
    let mut total = 0usize;

    if !dir.data.is_null() {
        let prefix = dir.data as *const u8;
        while total < PATH_MAX - 2 {
            let c = *prefix.add(total);
            if c == 0 {
                break;
            }
            full_path_buf[total] = c;
            total += 1;
        }
        if total > 0 && total < PATH_MAX - 1 {
            full_path_buf[total] = b'/';
            total += 1;
        }
    }

    for &b in name {
        if total >= PATH_MAX {
            return Err(ENAMETOOLONG);
        }
        full_path_buf[total] = b;
        total += 1;
    }
    let child_path = &full_path_buf[..total];

    if let Some((_data, _size, entry_type)) = initramfs::initramfs_find(child_path) {
        if entry_type == b'5' {
            return Err(ENOENT);
        }
        match initramfs::initramfs_build_vnode(child_path) {
            Some(vn) => return Ok(vn),
            None => return Err(ENOMEM),
        }
    }

    Err(ENOENT)
}

//==============================================================================
// rootfs_init
//==============================================================================

/// Return a mutable reference to the rootfs root directory vnode.
pub unsafe fn rootfs_get_root() -> &'static mut Vnode {
    &mut ROOT_DIR
}

pub unsafe fn rootfs_init() {
    let root_ops = core::ptr::addr_of!(ROOTFS_DIR_OPS) as *const VnodeOps;

    ptr::write(&mut ROOT_DIR, Vnode {
        ops: root_ops,
        mode: V_DIR | 0o755,
        ino: 0,
        size: 0,
        fs_type: FsType::RootFS,
        data: ptr::null_mut(),
        mount: None,
    });
    ptr::write(&mut SBIN_DIR, Vnode {
        ops: root_ops,
        mode: V_DIR | 0o755,
        ino: 1,
        size: 0,
        fs_type: FsType::RootFS,
        data: b"sbin\0".as_ptr() as *mut (),
        mount: None,
    });
    ptr::write(&mut BIN_DIR, Vnode {
        ops: root_ops,
        mode: V_DIR | 0o755,
        ino: 2,
        size: 0,
        fs_type: FsType::RootFS,
        data: b"bin\0".as_ptr() as *mut (),
        mount: None,
    });
    ptr::write(&mut ETC_DIR, Vnode {
        ops: root_ops,
        mode: V_DIR | 0o755,
        ino: 3,
        size: 0,
        fs_type: FsType::RootFS,
        data: b"etc\0".as_ptr() as *mut (),
        mount: None,
    });
    ptr::write(&mut DEV_DIR, Vnode {
        ops: root_ops,
        mode: V_DIR | 0o755,
        ino: 4,
        size: 0,
        fs_type: FsType::RootFS,
        data: ptr::null_mut(),
        mount: None,
    });
}
