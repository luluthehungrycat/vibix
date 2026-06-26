//==============================================================================
// vfs/chardev.rs — Character device registration framework
//
// Provides a registry that maps device names to numeric device IDs.
// DevFS uses this to look up devices by name, avoiding hardcoded if-else chains.
//==============================================================================

use crate::vfs::ENFILE;

const MAX_CHARDEVS: usize = 8;

#[repr(C)]
pub struct Chardev {
    pub name: [u8; 32],
    pub name_len: usize,
    pub dev_id: u32,
}

static mut CHARDEV_TABLE: [Chardev; MAX_CHARDEVS] = [
    Chardev { name: [0u8; 32], name_len: 0, dev_id: 0 },
    Chardev { name: [0u8; 32], name_len: 0, dev_id: 0 },
    Chardev { name: [0u8; 32], name_len: 0, dev_id: 0 },
    Chardev { name: [0u8; 32], name_len: 0, dev_id: 0 },
    Chardev { name: [0u8; 32], name_len: 0, dev_id: 0 },
    Chardev { name: [0u8; 32], name_len: 0, dev_id: 0 },
    Chardev { name: [0u8; 32], name_len: 0, dev_id: 0 },
    Chardev { name: [0u8; 32], name_len: 0, dev_id: 0 },
];
static mut CHARDEV_COUNT: usize = 0;

/// Register a character device by name.
/// Returns 0 on success, -ENFILE if the registry is full.
pub unsafe fn register_chardev(name: &[u8], dev_id: u32) -> i32 {
    let count = CHARDEV_COUNT;
    if count >= MAX_CHARDEVS {
        return -ENFILE;
    }
    let entry = &mut CHARDEV_TABLE[count];
    let copy_len = name.len().min(31);
    entry.name[..copy_len].copy_from_slice(&name[..copy_len]);
    entry.name[copy_len] = 0;
    entry.name_len = copy_len;
    entry.dev_id = dev_id;
    CHARDEV_COUNT = count + 1;
    0
}

/// Look up a character device by name.
/// Returns the dev_id if found, None otherwise.
pub unsafe fn chardev_find(name: &[u8]) -> Option<u32> {
    let count = CHARDEV_COUNT;
    for i in 0..count {
        let entry = &CHARDEV_TABLE[i];
        let entry_name = core::slice::from_raw_parts(
            entry.name.as_ptr(),
            entry.name_len,
        );
        if entry_name == name {
            return Some(entry.dev_id);
        }
    }
    None
}
