//==============================================================================
// kmm.rs — Kernel Heap Allocator (first-fit free list)
//
// Provides kmalloc/kfree for arbitrary-sized kernel heap allocations, backed
// by PMM pages.
//
// Layout of each block:
//   [ BlockHeader | usable data ]
//   ^             ^
//   header        returned ptr
//
// Free blocks form a singly-linked list sorted by address (ascending).
// Allocation: first-fit scan; split if remainder ≥ MIN_BLOCK_SIZE.
// Free: insert by address, coalesce with adjacent free blocks.
//==============================================================================

use core::ptr;
use crate::pmm;
use crate::pmm::PmmAllocator;

//--- Constants ---------------------------------------------------------------

/// Magic number stored in every valid block header.
const MAGIC: u32 = 0x564B_4841; // "VKHA" — Vibix Kernel Heap Allocator

/// Minimum block size (incl. header) that we'll split into two blocks.
/// Prevents creating unuseably tiny fragments.
const MIN_BLOCK_SIZE: usize = 64;

/// Alignment for all returned allocations.
const KMALLOC_ALIGN: usize = 8;

/// Header overhead (magic + padding + size + next = 4 + 4 + 8 + 8 = 24).
const HEADER_SIZE: usize = core::mem::size_of::<BlockHeader>();

/// Number of 4K pages for the initial heap pool.
const INITIAL_HEAP_PAGES: usize = 256; // 1 MB

//--- Block Header ------------------------------------------------------------

#[repr(C)]
struct BlockHeader {
    /// Magic number for corruption detection.
    magic: u32,
    /// Total block size (including this header).
    size: usize,
    /// Next free block in the list (only meaningful while free).
    next: *mut BlockHeader,
}

impl BlockHeader {
    /// Return the block header immediately following this one in memory.
    #[allow(dead_code)]
    fn next_block(&self) -> *mut BlockHeader {
        (self as *const Self as usize + self.size) as *mut BlockHeader
    }

    /// Return pointer to usable data (right after the header).
    fn data_ptr(&self) -> *mut u8 {
        (self as *const Self as usize + HEADER_SIZE) as *mut u8
    }

    /// Check whether this block covers a given address (for coalescing).
    #[allow(dead_code)]
    fn contains(&self, addr: usize) -> bool {
        let start = self as *const Self as usize;
        addr >= start && addr < start + self.size
    }

    /// The address immediately after this block ends.
    fn end(&self) -> usize {
        self as *const Self as usize + self.size
    }
}

//--- Global Allocator State --------------------------------------------------
//
// The simple approach: a static free-list head, plus heap bounds for
// detection of out-of-heap conditions. The PMM reference is consumed at
// init time and never stored globally.

static mut FREE_LIST: *mut BlockHeader = ptr::null_mut();
static mut HEAP_LOW: usize = 0;
static mut HEAP_HIGH: usize = 0;

//--- Core Allocator Functions ------------------------------------------------

/// Initialise the kernel heap by allocating INITIAL_HEAP_PAGES from the PMM.
/// Must be called once, after PMM is ready and before any kmalloc/kfree.
pub fn init(pmm: &mut PmmAllocator) {
    let heap_start = pmm.alloc_pages(INITIAL_HEAP_PAGES);
    if heap_start.is_null() {
        // No memory — the kernel can't function without a heap.
        // In a real kernel this would panic.  Here we leave HEAP_LOW/HEAP_HIGH
        // at zero so all subsequent kmalloc returns NULL.
        return;
    }

    let heap_base = heap_start as usize;
    let heap_size = INITIAL_HEAP_PAGES * pmm::PMM_PAGE_SIZE;

    unsafe {
        HEAP_LOW = heap_base;
        HEAP_HIGH = heap_base + heap_size;

        // Initialise a single free block covering the whole heap.
        let block = heap_base as *mut BlockHeader;
        (*block).magic = MAGIC;
        (*block).size = heap_size;
        (*block).next = ptr::null_mut();
        FREE_LIST = block;
    }
}

/// Allocate `size` bytes from the kernel heap.
/// Returns a pointer to usable memory, or null on failure.
pub fn kmalloc(size: usize) -> *mut u8 {
    if size == 0 {
        return ptr::null_mut();
    }

    unsafe {
        let mut prev: *mut BlockHeader = ptr::null_mut();
        let mut curr = FREE_LIST;

        // Round size up to KMALLOC_ALIGN.
        let aligned_size = (size + KMALLOC_ALIGN - 1) & !(KMALLOC_ALIGN - 1);
        let needed = HEADER_SIZE + aligned_size;

        // First-fit walk.
        while !curr.is_null() {
            let block_size = (*curr).size;
            if block_size >= needed {
                // Split if the remainder is large enough to be useful.
                let remainder = block_size - needed;
                if remainder >= MIN_BLOCK_SIZE {
                    // The allocated portion stays in-place.
                    (*curr).size = needed;
                    // Create a new free block after it.
                    let new_free = (curr as usize + needed) as *mut BlockHeader;
                    (*new_free).magic = MAGIC;
                    (*new_free).size = remainder;
                    (*new_free).next = (*curr).next;
                    (*curr).next = new_free;

                    // Update or bypass: if the current block was also the
                    // free-list head, FREE_LIST may still point to it directly.
                    // If we split, the current block (allocated) leaves the
                    // free list.  The new free block takes its "next" position.
                    if prev.is_null() {
                        FREE_LIST = new_free;
                    } else {
                        (*prev).next = new_free;
                    }
                } else {
                    // Take the whole block (no split).
                    if prev.is_null() {
                        FREE_LIST = (*curr).next;
                    } else {
                        (*prev).next = (*curr).next;
                    }
                }

                // Mark allocated and return data pointer.
                (*curr).magic = MAGIC;
                return (*curr).data_ptr();
            }

            prev = curr;
            curr = (*curr).next;
        }
    }

    // Out of memory — no suitable block found.
    ptr::null_mut()
}

/// Free a block previously returned by kmalloc.
/// Does nothing if ptr is null.
pub fn kfree(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }

    unsafe {
        // Recover the block header.
        let block = (ptr as usize - HEADER_SIZE) as *mut BlockHeader;

        // Sanity check.
        if (*block).magic != MAGIC {
            // Corrupted heap — silently ignore (in a real kernel we'd panic).
            return;
        }

        // Walk the free list (sorted by address) to find insertion point.
        let mut prev: *mut BlockHeader = ptr::null_mut();
        let mut curr = FREE_LIST;

        while !curr.is_null() && (curr as usize) < (block as usize) {
            prev = curr;
            curr = (*curr).next;
        }

        // Coalesce with the next free block (curr) if adjacent.
        if !curr.is_null() && (*block).end() == curr as usize {
            (*block).size += (*curr).size;
            (*block).next = (*curr).next;
        } else {
            (*block).next = curr;
        }

        // Coalesce with the previous free block (prev) if adjacent.
        if !prev.is_null() && (*prev).end() == block as usize {
            (*prev).size += (*block).size;
            (*prev).next = (*block).next;
        } else if prev.is_null() {
            FREE_LIST = block;
        } else {
            (*prev).next = block;
        }
    }
}

//--- Diagnostics -------------------------------------------------------------

/// Print current heap usage statistics to the serial port.
#[allow(dead_code)]
pub fn dump_stats(serial: &mut crate::serial::SerialPort) {
    unsafe {
        let free_bytes = compute_free_bytes();
        let total_bytes = HEAP_HIGH - HEAP_LOW;
        let used_bytes = total_bytes - free_bytes;

        serial.writestrs(&["KMM: Heap 0x"]);
        write_hex(serial, HEAP_LOW);
        serial.writestrs(&["-0x"]);
        write_hex(serial, HEAP_HIGH);
        serial.writestrs(&[" (total "]);
        write_dec(serial, total_bytes);
        serial.writestrs(&[" bytes)\n"]);

        serial.writestrs(&["KMM: Used "]);
        write_dec(serial, used_bytes);
        serial.writestrs(&[" / Free "]);
        write_dec(serial, free_bytes);
        serial.writestrs(&[" bytes\n"]);

        // Count free blocks.
        let mut count = 0usize;
        let mut curr = FREE_LIST;
        while !curr.is_null() {
            count += 1;
            curr = (*curr).next;
        }
        serial.writestrs(&["KMM: Free-list entries: "]);
        write_dec(serial, count);
        serial.writestrs(&["\n"]);
    }
}

/// Quick self-test: allocate, write patterns, free, verify coalescing.
pub fn test(serial: &mut crate::serial::SerialPort) {
    let p1 = kmalloc(32);
    let p2 = kmalloc(64);
    let p3 = kmalloc(128);

    if p1.is_null() || p2.is_null() || p3.is_null() {
        serial.writestrs(&["KMM: Test FAILED — allocation returned null.\n"]);
        return;
    }

    // Write identifiable patterns.
    unsafe {
        *(p1 as *mut u64) = 0xDEAD_BEEF_CAFE_BABEu64;
        *(p2 as *mut u64) = 0x1234_5678_9ABC_DEF0u64;
        *(p3 as *mut u64) = 0x0F0F_0F0F_F0F0_F0F0u64;
    }

    // Free the middle one, then allocate again — should reuse p2.
    kfree(p2);
    let p4 = kmalloc(64);
    if p4 == p2 {
        serial.writestrs(&["KMM: Test OK — freed block reused.\n"]);
    } else {
        serial.writestrs(&["KMM: Test FAILED — reuse address mismatch.\n"]);
    }

    // Clean up remaining.
    kfree(p1);
    kfree(p3);
    kfree(p4);

    // Verify free list is back to one large block.
    unsafe {
        let mut count = 0usize;
        let mut curr = FREE_LIST;
        while !curr.is_null() {
            count += 1;
            curr = (*curr).next;
        }
        if count == 1 {
            serial.writestrs(&["KMM: Coalescing OK — single free block.\n"]);
        } else {
            serial.writestrs(&["KMM: Coalescing WARNING — "]);
            write_dec(serial, count);
            serial.writestrs(&[" free blocks (expected 1).\n"]);
        }
    }
}

#[allow(dead_code)]
fn compute_free_bytes() -> usize {
    unsafe {
        let mut total = 0usize;
        let mut curr = FREE_LIST;
        while !curr.is_null() {
            total += (*curr).size;
            curr = (*curr).next;
        }
        total
    }
}

#[allow(dead_code)]
fn write_hex(serial: &mut crate::serial::SerialPort, val: usize) {
    let mut buf = [0u8; 18];
    buf[0] = b'0'; buf[1] = b'x';
    for i in 0..16 {
        let nibble = (val >> (60 - 4 * i)) & 0xF;
        buf[2 + i] = if nibble < 10 { b'0' + nibble as u8 } else { b'a' + nibble as u8 - 10 };
    }
    // Trim leading zeros.
    let mut start = 2;
    while start < 17 && buf[start] == b'0' { start += 1; }
    let slice = if start < 18 { &buf[start..18] } else { &buf[17..18] };
    for &b in slice {
        serial.putchar(b as char);
    }
}

fn write_dec(serial: &mut crate::serial::SerialPort, mut val: usize) {
    let mut buf = [0u8; 20];
    let mut i = 20;
    if val == 0 {
        serial.putchar('0');
        return;
    }
    while val > 0 {
        i -= 1;
        buf[i] = b'0' + (val % 10) as u8;
        val /= 10;
    }
    for &b in &buf[i..20] {
        serial.putchar(b as char);
    }
}
