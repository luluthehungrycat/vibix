//==============================================================================
// multiboot.rs — Multiboot v1 info structure parser
//
// Reads the Multiboot info structure saved by boot.asm at the fixed physical
// address 0x5000, then parses the memory map to discover physical RAM layout.
//==============================================================================

use crate::serial::SerialPort;
use crate::pmm::PmmAllocator;

//--- Fixed physical address where boot.asm saves the MBI pointer ------------
const MBI_PTR_ADDR: *const u32 = 0x5000 as *const u32;

/// Return the Multiboot info pointer saved by boot.asm (0 if unavailable).
pub fn get_mbi_ptr() -> u32 {
    unsafe { *MBI_PTR_ADDR }
}

/// Check whether the MBI has a valid memory map (flags bit 6).
fn has_mmap(mbi: u32) -> bool {
    if mbi == 0 {
        return false;
    }
    let flags = unsafe { *(mbi as *const u32) };
    flags & (1 << 6) != 0
}

/// A single Multiboot memory map entry (v1 format — 24 bytes).
#[repr(C, packed)]
struct MmapEntry {
    size: u32,       // byte size of entry excluding the size field itself
    base_low: u32,
    base_high: u32,
    length_low: u32,
    length_high: u32,
    region_type: u32, // 1 = available, 2 = reserved, 3 = ACPI reclaim, 4 = NVS
}

/// Print the memory map to serial for debugging.
pub fn print_mmap(mbi: u32, serial: &mut SerialPort) {
    if !has_mmap(mbi) {
        serial.writestrs(&["VIBIX: No Multiboot memory map available.\n"]);
        return;
    }

    let mmap_length = unsafe { *((mbi as usize + 44) as *const u32) };
    let mmap_addr   = unsafe { *((mbi as usize + 48) as *const u32) };

    serial.writestrs(&["VIBIX: Multiboot memory map:\n"]);

    let mut offset: u32 = 0;
    while offset < mmap_length {
        let entry = unsafe { &*((mmap_addr + offset) as *const MmapEntry) };
        let entry_size = entry.size + 4;

        let base = (entry.base_low as u64) | ((entry.base_high as u64) << 32);
        let length = (entry.length_low as u64) | ((entry.length_high as u64) << 32);
        let type_name = match entry.region_type {
            1 => "Available",
            2 => "Reserved",
            3 => "ACPI reclaim",
            4 => "ACPI NVS",
            _ => "Bad RAM",
        };

        let base_hi = hex_buf((base >> 32) as u32);
        let base_lo = hex_buf(base as u32);
        let len_hi = hex_buf((length >> 32) as u32);
        let len_lo = hex_buf(length as u32);

        serial.writestrs(&["  base="]);
        serial.writestrs(&[unsafe { core::str::from_utf8_unchecked(&base_hi) }]);
        serial.writestrs(&["_"]);
        serial.writestrs(&[unsafe { core::str::from_utf8_unchecked(&base_lo) }]);
        serial.writestrs(&["  len="]);
        serial.writestrs(&[unsafe { core::str::from_utf8_unchecked(&len_hi) }]);
        serial.writestrs(&["_"]);
        serial.writestrs(&[unsafe { core::str::from_utf8_unchecked(&len_lo) }]);
        serial.writestrs(&["  ", type_name, "\n"]);

        offset += entry_size;
    }
}

/// Apply the Multiboot memory map to the PMM:
/// 1. Determine PMM range from the lowest Available region
/// 2. Mark all pages USED by default
/// 3. Mark only Available regions as FREE via init_region
///
/// Returns true if the mmap was applied, false if fallback to hardcoded range.
pub fn apply_mmap_to_pmm(pmm: &mut PmmAllocator, mbi: u32) -> bool {
    if !has_mmap(mbi) {
        return false;
    }

    let mmap_length = unsafe { *((mbi as usize + 44) as *const u32) };
    let mmap_addr   = unsafe { *((mbi as usize + 48) as *const u32) };

    // Find the first Available region to set memory_start
    let mut first_avail_base: Option<u64> = None;
    let mut highest_end: u64 = 0;

    let mut offset: u32 = 0;
    while offset < mmap_length {
        let entry = unsafe { &*((mmap_addr + offset) as *const MmapEntry) };
        let entry_size = entry.size + 4;

        if entry.region_type == 1 {
            let base = (entry.base_low as u64) | ((entry.base_high as u64) << 32);
            let length = (entry.length_low as u64) | ((entry.length_high as u64) << 32);
            if first_avail_base.is_none() {
                first_avail_base = Some(base);
            }
            let end = base + length;
            if end > highest_end {
                highest_end = end;
            }
        }
        offset += entry_size;
    }

    let memory_start = match first_avail_base {
        Some(b) => {
            // Start at 1 MB at minimum (skip low memory: IVT, BDA, EBDA, boot code).
            let page_base = (b & !0xFFF) as usize;
            if page_base < 0x100000 { 0x100000 } else { page_base }
        }
        None => return false,
    };

    // Cap at 256 MB or the highest region, whichever is lower
    let max_end = 0x1000_0000usize; // 256 MB
    let memory_end = (highest_end as usize).min(max_end);

    // Initialize PMM with all pages marked USED
    pmm.init_all_used(memory_start, memory_end);

    // Now mark each Available region as free
    let mut offset: u32 = 0;
    while offset < mmap_length {
        let entry = unsafe { &*((mmap_addr + offset) as *const MmapEntry) };
        let entry_size = entry.size + 4;

        if entry.region_type == 1 {
            let base = (entry.base_low as u64) | ((entry.base_high as u64) << 32);
            let length = (entry.length_low as u64) | ((entry.length_high as u64) << 32);

            // Clamp to PMM range
            let clamp_base = if base < memory_start as u64 { memory_start as u64 } else { base };
            let clamp_end = core::cmp::min(base + length, memory_end as u64);
            if clamp_end > clamp_base {
                pmm.init_region(clamp_base as usize, (clamp_end - clamp_base) as usize);
            }
        }
        offset += entry_size;
    }

    true
}

//--- Hex helpers ------------------------------------------------------------

/// Format a 32-bit value as "0x........" in a fixed 10-byte buffer.
fn hex_buf(val: u32) -> [u8; 10] {
    let hex = b"0123456789ABCDEF";
    let mut buf = [0u8; 10];
    buf[0] = b'0';
    buf[1] = b'x';
    for i in 0..8 {
        let nibble = ((val >> (28 - i * 4)) & 0xF) as usize;
        buf[i + 2] = hex[nibble];
    }
    buf
}


