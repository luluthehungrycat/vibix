//==============================================================================
// vfs/open_file.rs — Open file descriptions and Open File Table (OFT)
//
// The OFT is the system-global table of open file descriptions.
// Each process's fd table points into this table (indices).
//
// Mutable statics follow the PROCESS_TABLE pattern: accessed only
// with interrupts disabled.
//==============================================================================

use crate::vfs;
use crate::vfs::Vnode;
use crate::vfs::OFT_SIZE;

//==============================================================================
// Dirent — directory entry returned by getdents
//==============================================================================

#[repr(C)]
pub struct Dirent {
    pub d_ino: u64,
    pub d_off: i64,
    pub d_reclen: u16,
    pub d_type: u8,
    pub d_name: [u8; 256],
}

//==============================================================================
// OpenFile — system-global open file description
//==============================================================================

#[repr(C)]
pub struct OpenFile {
    pub vnode: Option<&'static mut Vnode>,
    pub offset: u64,
    pub flags: u32,
    pub mode: u32,
    pub refcount: u32,
}

//==============================================================================
// OpenFileTable — fixed-size array of open file descriptions
//==============================================================================

#[repr(C)]
pub struct OpenFileTable {
    pub entries: [Option<OpenFile>; OFT_SIZE],
}

impl OpenFileTable {
    pub const fn new() -> Self {
        const NONE: Option<OpenFile> = None;
        OpenFileTable {
            entries: [NONE; OFT_SIZE],
        }
    }
}

/// Singleton — accessed only with interrupts disabled.
pub static mut OPEN_FILE_TABLE: OpenFileTable = OpenFileTable::new();

//==============================================================================
// OFT operations
//==============================================================================

/// Allocate a new open file description in the OFT.
///
/// Returns the slot index on success, or `Err(ENFILE)` if the table is full.
pub fn oft_alloc(vnode: &'static mut Vnode, flags: u32, mode: u32) -> Result<usize, i32> {
    unsafe {
        for i in 0..OFT_SIZE {
            if OPEN_FILE_TABLE.entries[i].is_none() {
                OPEN_FILE_TABLE.entries[i] = Some(OpenFile {
                    vnode: Some(vnode),
                    offset: 0,
                    flags,
                    mode,
                    refcount: 1,
                });
                return Ok(i);
            }
        }
    }
    Err(vfs::ENFILE)
}

/// Decrement the reference count of the open file at `index`.
///
/// If refcount reaches 0, the entry is freed and the vnode's `close` op
/// is called (if it exists).
pub fn oft_decref(index: usize) {
    unsafe {
        if index >= OFT_SIZE {
            return;
        }
        let entry = &mut OPEN_FILE_TABLE.entries[index];
        if let Some(ref mut of) = entry {
            if of.refcount > 0 {
                of.refcount -= 1;
            }
            if of.refcount == 0 {
                // Call vnode close op if present before freeing
                if let Some(ref mut vn) = of.vnode {
                    if let Some(close) = (*(*vn).ops).close {
                        let _ = close(&mut **vn);
                    }
                }
                *entry = None;
            }
        }
    }
}

/// Increment the reference count of the open file at `index`.
///
/// Used by fork() and dup() when an fd is shared between processes.
pub fn oft_incref(index: usize) {
    unsafe {
        if index < OFT_SIZE {
            if let Some(ref mut of) = OPEN_FILE_TABLE.entries[index] {
                of.refcount = of.refcount.saturating_add(1);
            }
        }
    }
}

/// Get a mutable reference to the open file at `index`.
pub fn oft_get(index: usize) -> Option<&'static mut OpenFile> {
    unsafe {
        if index >= OFT_SIZE {
            return None;
        }
        match &mut OPEN_FILE_TABLE.entries[index] {
            Some(ref mut of) => Some(of),
            None => None,
        }
    }
}


