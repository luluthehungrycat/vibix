//==============================================================================
// vfs/mount.rs — Mount table management
//
// Manages a fixed-size mount table that maps mount points to filesystem
// root vnodes.
//
// Mutable statics follow the PROCESS_TABLE pattern: accessed only
// with interrupts disabled.
//==============================================================================

use crate::vfs::Vnode;
use crate::vfs::MAX_MOUNTS;
use crate::vfs::FsType;
use crate::vfs::ENOMEM;

//==============================================================================
// MountEntry — a single mounted filesystem
//==============================================================================

#[repr(C)]
pub struct MountEntry {
    pub path: &'static str,
    pub root: Option<&'static mut Vnode>,
    pub fs_type: FsType,
    pub flags: u32,
}

//==============================================================================
// MountTable — fixed-size array of mount entries
//==============================================================================

#[repr(C)]
pub struct MountTable {
    pub entries: [Option<MountEntry>; MAX_MOUNTS],
    pub count: usize,
}

impl MountTable {
    pub const fn new() -> Self {
        const NONE: Option<MountEntry> = None;
        MountTable {
            entries: [NONE; MAX_MOUNTS],
            count: 0,
        }
    }
}

pub static mut MOUNT_TABLE: MountTable = MountTable::new();

//==============================================================================
// Mount operations
//==============================================================================

pub fn mount_add(
    path: &'static str,
    root: &'static mut Vnode,
    fs_type: FsType,
    flags: u32,
) -> i32 {
    unsafe {
        if MOUNT_TABLE.count >= MAX_MOUNTS {
            return -ENOMEM;
        }
        let idx = MOUNT_TABLE.count;
        MOUNT_TABLE.entries[idx] = Some(MountEntry {
            path,
            root: Some(root),
            fs_type,
            flags,
        });
        MOUNT_TABLE.count += 1;
    }
    0
}

/// Find a mount entry by matching its path string.
///
/// Returns the root vnode of the matching mount, or `None`.
/// Uses raw pointer transmutation to work around borrow checker
/// limitations with `&'static mut` inside `Option`.
pub fn mount_find(path: &[u8]) -> Option<&'static mut Vnode> {
    unsafe {
        for i in 0..MAX_MOUNTS {
            // Shared reference check for path matching
            let is_match = match &MOUNT_TABLE.entries[i] {
                Some(e) => {
                    let ep = e.path.as_bytes();
                    let eplen = if ep.last() == Some(&0) {
                        ep.len() - 1
                    } else {
                        ep.len()
                    };
                    path.len() == eplen && path == &ep[..eplen]
                }
                None => false,
            };
            if is_match {
                // Raw pointer cast to get &'static mut Vnode from shared ref
                let entry = &MOUNT_TABLE.entries[i];
                if let Some(ref e) = entry {
                    if let Some(ref vn) = e.root {
                        let raw = *vn as *const Vnode as *mut Vnode;
                        return Some(&mut *raw);
                    }
                }
            }
        }
    }
    None
}

/// Get the root vnode (first mount entry in the table).
pub fn mount_get_root() -> Option<&'static mut Vnode> {
    unsafe {
        if MOUNT_TABLE.count == 0 {
            return None;
        }
        let entry = &MOUNT_TABLE.entries[0];
        if let Some(ref e) = entry {
            if let Some(ref vn) = e.root {
                let raw = *vn as *const Vnode as *mut Vnode;
                return Some(&mut *raw);
            }
        }
    }
    None
}
