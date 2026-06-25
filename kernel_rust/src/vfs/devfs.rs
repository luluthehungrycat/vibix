//==============================================================================
// vfs/devfs.rs — Device filesystem
//
// Provides three static character devices:
//   /dev/null  — reads return 0, writes discard
//   /dev/zero  — reads fill buffer with zeros, writes discard
//   /dev/ttyS0 — reads from serial/keyboard, writes to serial
//
// Directory operations use DEVFS_DIR_OPS; device ops use DEVFS_OPS.
//==============================================================================

use core::ptr;
use crate::vfs::*;
use crate::vfs::open_file::Dirent;

//==============================================================================
// Device identifiers
//==============================================================================

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevId {
    Null  = 0,
    Zero  = 1,
    TtyS0 = 2,
}

//==============================================================================
// Static vnodes
//==============================================================================

static mut DEVFS_ROOT: Vnode = unsafe { core::mem::zeroed() };
static mut DEV_NULL_VNODE: Vnode = unsafe { core::mem::zeroed() };
static mut DEV_ZERO_VNODE: Vnode = unsafe { core::mem::zeroed() };
static mut DEV_TTYS0_VNODE: Vnode = unsafe { core::mem::zeroed() };

//==============================================================================
// Ops tables
//==============================================================================

static DEVFS_OPS: VnodeOps = VnodeOps {
    open:    Some(devfs_open as VnOpen),
    close:   Some(devfs_close as VnClose),
    read:    Some(devfs_read as VnRead),
    write:   Some(devfs_write as VnWrite),
    lseek:   Some(devfs_lseek as VnLseek),
    readdir: None,
    ioctl:   Some(devfs_ioctl as VnIoctl),
};

static DEVFS_DIR_OPS: VnodeOps = VnodeOps {
    open:    Some(devfs_open as VnOpen),
    close:   Some(devfs_close as VnClose),
    read:    None,
    write:   None,
    lseek:   None,
    readdir: Some(devfs_readdir as VnReaddir),
    ioctl:   None,
};

//==============================================================================
// DevId extraction helper
//==============================================================================

unsafe fn dev_id(vnode: *mut Vnode) -> DevId {
    match (*vnode).data as u32 {
        0 => DevId::Null,
        1 => DevId::Zero,
        2 => DevId::TtyS0,
        _ => DevId::Null,
    }
}

//==============================================================================
// Vnode operation implementations
//==============================================================================

unsafe fn devfs_open(_vnode: *mut Vnode, _flags: u32, _mode: u32) -> i32 { 0 }
unsafe fn devfs_close(_vnode: *mut Vnode) -> i32 { 0 }

unsafe fn devfs_read(vnode: *mut Vnode, buf: *mut u8, len: usize, _offset: &mut u64) -> isize {
    match dev_id(vnode) {
        DevId::Null => 0,
        DevId::Zero => {
            ptr::write_bytes(buf, 0, len);
            len as isize
        }
        DevId::TtyS0 => {
            let slice = core::slice::from_raw_parts_mut(buf, len);
            let mut count = crate::keyboard::read(slice);
            if count == 0 {
                let serial = crate::serial::SerialPort::new();
                count = serial.read(slice);
            }
            count as isize
        }
    }
}

unsafe fn devfs_write(vnode: *mut Vnode, buf: *const u8, len: usize, _offset: &mut u64) -> isize {
    match dev_id(vnode) {
        DevId::Null | DevId::Zero => len as isize,
        DevId::TtyS0 => {
            let mut serial = crate::serial::SerialPort::new();
            let slice = core::slice::from_raw_parts(buf, len);
            for &byte in slice {
                serial.putchar(byte as char);
            }
            len as isize
        }
    }
}

unsafe fn devfs_lseek(vnode: *mut Vnode, offset: i64, whence: u32) -> i64 {
    match dev_id(vnode) {
        DevId::TtyS0 => -ESPIPE as i64,
        _ => match whence {
            0 => offset,
            1 => offset,
            2 => ((*vnode).size as i64).wrapping_add(offset),
            _ => -EINVAL as i64,
        },
    }
}

unsafe fn devfs_readdir(_vnode: *mut Vnode, dirent: *mut Dirent, index: u32) -> i32 {
    if dirent.is_null() {
        return -EFAULT;
    }
    let (name_bytes, dtype): (&[u8], u16) = match index {
        0 => (b".\0", V_DIR),
        1 => (b"..\0", V_DIR),
        2 => (b"null\0", V_CHR),
        3 => (b"zero\0", V_CHR),
        4 => (b"ttyS0\0", V_CHR),
        _ => return 0,
    };
    let de = &mut *dirent;
    de.d_ino = (index + 1) as u64;
    de.d_off = 0;
    de.d_reclen = core::mem::size_of::<Dirent>() as u16;
    de.d_type = dtype as u8;
    let mut i = 0usize;
    while i < name_bytes.len() && i < 256 {
        de.d_name[i] = name_bytes[i];
        i += 1;
    }
    while i < 256 {
        de.d_name[i] = 0;
        i += 1;
    }
    1
}

unsafe fn devfs_ioctl(vnode: *mut Vnode, request: u64, arg: u64) -> i32 {
    if dev_id(vnode) != DevId::TtyS0 {
        return -ENOTTY;
    }
    if arg == 0 {
        return -EFAULT;
    }
    match request {
        // TIOCGWINSZ (0x5413): Get window size — return 80x25
        0x5413 => {
            // struct winsize { u16 ws_row; u16 ws_col; u16 ws_xpixel; u16 ws_ypixel; }
            let winsize: [u16; 4] = [25, 80, 0, 0];
            ptr::copy_nonoverlapping(winsize.as_ptr(), arg as *mut u16, 4);
            0
        }
        // TCGETS (0x5401): Get terminal attributes
        0x5401 => {
            let mut termios = [0u8; 60];
            // c_cflag at offset 8: B9600 | CS8 | CREAD | CLOCAL = 0xBFD
            termios[8..12].copy_from_slice(&[0xFD, 0x0B, 0x00, 0x00]);
            // c_lflag at offset 12: ICANON | ECHO | ISIG = 0x3CB
            termios[12..16].copy_from_slice(&[0xCB, 0x03, 0x00, 0x00]);
            ptr::copy_nonoverlapping(termios.as_ptr(), arg as *mut u8, 60);
            0
        }
        // TCSETS (0x5402): Set terminal attributes — no-op
        0x5402 => 0,
        _ => -ENOTTY,
    }
}

//==============================================================================
// devfs_lookup
//==============================================================================

pub unsafe fn devfs_lookup(name: &[u8]) -> Option<&'static mut Vnode> {
    let dev_id_val = crate::vfs::chardev::chardev_find(name)?;
    let id = match dev_id_val {
        0 => DevId::Null,
        1 => DevId::Zero,
        2 => DevId::TtyS0,
        _ => return None,
    };
    Some(devfs_get_vnode(id))
}

//==============================================================================
// devfs_init
//==============================================================================

pub unsafe fn devfs_init() -> &'static mut Vnode {
    ptr::write(&mut DEV_NULL_VNODE, Vnode {
        ops: core::ptr::addr_of!(DEVFS_OPS) as *const VnodeOps,
        mode: V_CHR | 0o666,
        ino: 1,
        size: 0,
        fs_type: FsType::DevFS,
        data: (DevId::Null as u32) as *mut (),
        mount: None,
    });

    ptr::write(&mut DEV_ZERO_VNODE, Vnode {
        ops: core::ptr::addr_of!(DEVFS_OPS) as *const VnodeOps,
        mode: V_CHR | 0o666,
        ino: 2,
        size: 0,
        fs_type: FsType::DevFS,
        data: (DevId::Zero as u32) as *mut (),
        mount: None,
    });

    ptr::write(&mut DEV_TTYS0_VNODE, Vnode {
        ops: core::ptr::addr_of!(DEVFS_OPS) as *const VnodeOps,
        mode: V_CHR | 0o666,
        ino: 3,
        size: 0,
        fs_type: FsType::DevFS,
        data: (DevId::TtyS0 as u32) as *mut (),
        mount: None,
    });

    ptr::write(&mut DEVFS_ROOT, Vnode {
        ops: core::ptr::addr_of!(DEVFS_DIR_OPS) as *const VnodeOps,
        mode: V_DIR | 0o755,
        ino: 0,
        size: 0,
        fs_type: FsType::DevFS,
        data: ptr::null_mut(),
        mount: None,
    });


    // Register char devices in the chardev registry
    crate::vfs::chardev::register_chardev(b"null", DevId::Null as u32);
    crate::vfs::chardev::register_chardev(b"zero", DevId::Zero as u32);
    crate::vfs::chardev::register_chardev(b"ttyS0", DevId::TtyS0 as u32);
    &mut DEVFS_ROOT
}

//==============================================================================
// devfs_get_vnode — return static vnode for a DevId
//==============================================================================

pub unsafe fn devfs_get_vnode(id: DevId) -> &'static mut Vnode {
    match id {
        DevId::Null  => &mut DEV_NULL_VNODE,
        DevId::Zero  => &mut DEV_ZERO_VNODE,
        DevId::TtyS0 => &mut DEV_TTYS0_VNODE,
    }
}
