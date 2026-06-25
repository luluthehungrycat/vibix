//==============================================================================
// vfs/initramfs.rs — initramfs ustar archive parser
//
// Parses a ustar-format tar archive embedded at link time.
// Each entry's file data is referenced in-place (no copy).
//
// For Phase 1, the global archive pointer is null and parsing is skipped.
// Phase 2 will wire up an embedded archive via linker script.
//
// Vnodes for initramfs-backed files are allocated from kmalloc (they live
// forever — never freed in Phase 1).
//
// Mutable statics follow the PROCESS_TABLE pattern: accessed only
// with interrupts disabled.
//==============================================================================

use core::ptr;
use core::mem;
use crate::vfs::*;
use crate::kmm;

//==============================================================================
// Constants
//==============================================================================

const USTAR_MAGIC: &[u8; 6] = b"ustar\0";
const USTAR_BLOCK_SIZE: usize = 512;
const MAX_INITRAMFS_ENTRIES: usize = 128;

//==============================================================================
// InitramfsEntry — parsed entry from the ustar archive
//==============================================================================

#[repr(C)]
pub struct InitramfsEntry {
    pub name: *const u8,
    pub data: *const u8,
    pub size: u64,
    pub entry_type: u8,
}

//==============================================================================
// Global initramfs state
//==============================================================================

static mut ARCHIVE_DATA: *const u8 = ptr::null();
static mut ARCHIVE_LEN: usize = 0;

static mut INITRAMFS_ENTRIES: *mut InitramfsEntry = ptr::null_mut();
static mut INITRAMFS_COUNT: usize = 0;

pub unsafe fn initramfs_set_archive(data: *const u8, len: usize) {
    ARCHIVE_DATA = data;
    ARCHIVE_LEN = len;
}

//==============================================================================
// Ops table for initramfs regular files (read-only)
//==============================================================================

static INITRAMFS_FILE_OPS: VnodeOps = VnodeOps {
    open:    None,
    close:   None,
    read:    Some(initramfs_read as VnRead),
    write:   None,
    lseek:   None,
    readdir: None,
    ioctl:   None,
};

//==============================================================================
// Ustar header parsing helpers
//==============================================================================

fn parse_octal(bytes: &[u8]) -> u64 {
    let mut val: u64 = 0;
    for &b in bytes {
        if b >= b'0' && b <= b'7' {
            val = val.wrapping_mul(8).wrapping_add((b - b'0') as u64);
        } else if b == 0 || b == b' ' {
            break;
        }
    }
    val
}

fn is_valid_header(block: &[u8; 512]) -> bool {
    block[257..263] == *USTAR_MAGIC
}

fn is_zero_block(block: &[u8; 512]) -> bool {
    for i in 0..512 {
        if block[i] != 0 {
            return false;
        }
    }
    true
}

fn header_name(block: &[u8; 512]) -> &[u8] {
    let mut len = 0usize;
    while len < 100 && block[len] != 0 {
        len += 1;
    }
    &block[..len]
}

fn header_size(block: &[u8; 512]) -> u64 {
    parse_octal(&block[124..136])
}

fn header_typeflag(block: &[u8; 512]) -> u8 {
    block[156]
}

//==============================================================================
// Initialisation — parse the ustar archive
//==============================================================================

pub unsafe fn initramfs_init() {
    let arc: *const u8 = ARCHIVE_DATA;
    if arc.is_null() || ARCHIVE_LEN < USTAR_BLOCK_SIZE {
        INITRAMFS_COUNT = 0;
        INITRAMFS_ENTRIES = ptr::null_mut();
        return;
    }

    // First pass: count valid entries
    let mut count = 0usize;
    let mut off = 0usize;
    while off + USTAR_BLOCK_SIZE <= ARCHIVE_LEN {
        let block = &*(arc.add(off) as *const [u8; 512]);
        if is_zero_block(block) {
            break;
        }
        if !is_valid_header(block) {
            off += USTAR_BLOCK_SIZE;
            continue;
        }
        count += 1;
        let sz = header_size(block) as usize;
        off += USTAR_BLOCK_SIZE;
        if sz > 0 {
            off += (sz + USTAR_BLOCK_SIZE - 1) / USTAR_BLOCK_SIZE * USTAR_BLOCK_SIZE;
        }
        if count >= MAX_INITRAMFS_ENTRIES {
            break;
        }
    }
    if count == 0 {
        return;
    }

    let alloc_size = count * mem::size_of::<InitramfsEntry>();
    let entries_ptr = kmm::kmalloc(alloc_size) as *mut InitramfsEntry;
    if entries_ptr.is_null() {
        return;
    }

    // Second pass: fill entries
    let mut idx = 0usize;
    let mut off = 0usize;
    while off + USTAR_BLOCK_SIZE <= ARCHIVE_LEN && idx < count {
        let block = &*(arc.add(off) as *const [u8; 512]);
        if is_zero_block(block) {
            break;
        }
        if !is_valid_header(block) {
            off += USTAR_BLOCK_SIZE;
            continue;
        }

        let size_val = header_size(block) as usize;
        let typ = header_typeflag(block);
        let raw_name = header_name(block);
        let data_start = arc.add(off + USTAR_BLOCK_SIZE);

        ptr::write(entries_ptr.add(idx), InitramfsEntry {
            name: raw_name.as_ptr(),
            data: data_start,
            size: size_val as u64,
            entry_type: typ,
        });
        idx += 1;

        off += USTAR_BLOCK_SIZE;
        if size_val > 0 {
            off += (size_val + USTAR_BLOCK_SIZE - 1) / USTAR_BLOCK_SIZE * USTAR_BLOCK_SIZE;
        }
    }

    INITRAMFS_ENTRIES = entries_ptr;
    INITRAMFS_COUNT = idx;
}

//==============================================================================
// Lookup helpers
//==============================================================================

pub unsafe fn initramfs_find(path: &[u8]) -> Option<(*const u8, u64, u8)> {
    let entries = INITRAMFS_ENTRIES;
    let count = INITRAMFS_COUNT;
    if entries.is_null() || count == 0 {
        return None;
    }
    for i in 0..count {
        let entry = &*entries.add(i);
        if entry.name.is_null() {
            continue;
        }
        let mut j = 0usize;
        let mut matches = true;
        while j < path.len() {
            let ec = *entry.name.add(j);
            if ec == 0 {
                matches = false;
                break;
            }
            if ec != path[j] {
                matches = false;
                break;
            }
            j += 1;
        }
        if matches && *entry.name.add(j) == 0 {
            return Some((entry.data, entry.size, entry.entry_type));
        }
    }
    None
}

//==============================================================================
// Vnode read implementation
//==============================================================================

pub unsafe fn initramfs_read(
    vnode: *mut Vnode,
    buf: *mut u8,
    len: usize,
    offset: &mut u64,
) -> isize {
    let data = (*vnode).data as *const u8;
    let size = (*vnode).size;
    let remain = size.saturating_sub(*offset);
    let copy_len = (len as u64).min(remain) as usize;
    if copy_len == 0 {
        return 0;
    }
    ptr::copy_nonoverlapping(data.add(*offset as usize), buf, copy_len);
    *offset += copy_len as u64;
    copy_len as isize
}

//==============================================================================
// Vnode allocator (kmalloc-backed)
//==============================================================================

/// Allocate a new Vnode from the kernel heap for an initramfs-backed file.
///
/// Allocated vnodes live forever (never freed in Phase 1 MVP).
fn alloc_vnode() -> Option<&'static mut Vnode> {
    unsafe {
        let ptr = kmm::kmalloc(mem::size_of::<Vnode>()) as *mut Vnode;
        if ptr.is_null() {
            return None;
        }
        Some(&mut *ptr)
    }
}

/// Build a vnode for an initramfs entry found by path.
///
/// Returns `None` if the entry is not found or allocation fails.
pub unsafe fn initramfs_build_vnode(path: &[u8]) -> Option<&'static mut Vnode> {
    let (data, size, entry_type) = initramfs_find(path)?;
    let vn = alloc_vnode()?;
    let is_dir = entry_type == b'5';
    ptr::write(vn, Vnode {
        ops: &INITRAMFS_FILE_OPS as *const VnodeOps,
        mode: if is_dir { V_DIR | 0o755 } else { V_FILE | 0o644 },
        ino: 0,
        size,
        fs_type: FsType::InitramFS,
        data: data as *mut (),
        mount: None,
    });
    Some(vn)
}

//==============================================================================
// Embedded archive — included at compile time
//==============================================================================

/// Embedded ustar archive compiled into the kernel.
static EMBEDDED_ARCHIVE: &[u8] = include_bytes!("../../../userspace/initramfs.tar");

/// Call during vfs_init() before initramfs_init() to set the embedded archive.
pub fn initramfs_embed() {
    unsafe {
        initramfs_set_archive(EMBEDDED_ARCHIVE.as_ptr(), EMBEDDED_ARCHIVE.len());
    }
}
