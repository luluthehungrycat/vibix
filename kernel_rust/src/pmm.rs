//==============================================================================
// pmm.rs — Physical Memory Manager (bitmap allocator)
//
// Each bit represents one 4 KB page: 1 = allocated, 0 = free.
// The bitmap lives in statically-allocated BSS (zeroed by kernel64_entry.asm).
// Can track PMM_MAX_PAGES (64 K pages = 256 MB).
//==============================================================================

use crate::serial::SerialPort;

pub const PMM_PAGE_SIZE: usize = 4096;
pub const PMM_MAX_PAGES: usize = 0x10000;   // 64 K pages → 256 MB

const BITMAP_SIZE: usize = PMM_MAX_PAGES / 8;

/// The bitmap itself — lives in BSS (zeroed at boot by the entry stub).
//
// NOTE: we deliberately use `static mut` + raw-pointer access because Rust's
// borrow rules are unhelpful in single-threaded kernel space.  We never create
// a direct reference to the static — go through bitmap_mut() → raw ptr → deref.
static mut PMM_BITMAP: [u8; BITMAP_SIZE] = [0u8; BITMAP_SIZE];

/// Return a raw mutable pointer to the bitmap, avoiding a direct reference to
/// the `static mut` (which would trigger `static_mut_refs` in Rust 2024).
fn bitmap_mut() -> *mut [u8; BITMAP_SIZE] {
    &raw mut PMM_BITMAP
}

//--- PMM Allocator -----------------------------------------------------------

pub struct PmmAllocator {
    total_pages: usize,
}

impl PmmAllocator {
    pub const fn new() -> Self {
        Self { total_pages: 0 }
    }

    /// Initialise the PMM over a given physical memory region.
    pub fn init(&mut self, memory_start: usize, memory_end: usize) {
        let total_pages = (memory_end - memory_start) / PMM_PAGE_SIZE;
        self.total_pages = if total_pages > PMM_MAX_PAGES {
            PMM_MAX_PAGES
        } else {
            total_pages
        };

        // Zero the bitmap through raw pointer (avoids static_mut_refs warning).
        let bm = unsafe { &mut *bitmap_mut() };
        for slot in bm.iter_mut() {
            *slot = 0;
        }

        // Mark the first 4 pages as used (bitmap + early kernel/bootstrap).
        for i in 0..4 {
            bm[i / 8] |= 1 << (i % 8);
        }
    }

    /// Allocate a single 4 KB page.  Returns 0 (NULL) when exhausted.
    pub fn alloc(&mut self) -> *mut u8 {
        let bm = unsafe { &mut *bitmap_mut() };
        for i in 0..PMM_MAX_PAGES {
            if bm[i / 8] & (1 << (i % 8)) == 0 {
                bm[i / 8] |= 1 << (i % 8);
                return (i * PMM_PAGE_SIZE) as *mut u8;
            }
        }
        core::ptr::null_mut()
    }

    /// Free a previously-allocated page.
    pub fn free(&mut self, ptr: *mut u8) {
        let addr = ptr as usize;
        let page = addr / PMM_PAGE_SIZE;
        if page >= PMM_MAX_PAGES {
            return;
        }
        let bm = unsafe { &mut *bitmap_mut() };
        bm[page / 8] &= !(1 << (page % 8));
    }

    /// Mark a region of physical memory as usable (free).
    #[allow(dead_code)]
    pub fn init_region(&mut self, base: usize, size: usize) {
        let start_page = base / PMM_PAGE_SIZE;
        let end_page = if size == 0 {
            start_page
        } else {
            let end = (base + size - 1) / PMM_PAGE_SIZE;
            if end >= PMM_MAX_PAGES { PMM_MAX_PAGES - 1 } else { end }
        };

        let bm = unsafe { &mut *bitmap_mut() };
        for i in start_page..=end_page {
            bm[i / 8] &= !(1 << (i % 8));
        }
    }

    /// Quick self-test: allocate, free, reallocate, verify reuse.
    pub fn test(&mut self, serial: &mut SerialPort) {
        let p1 = self.alloc();
        let p2 = self.alloc();
        let p3 = self.alloc();

        if p1.is_null() || p2.is_null() || p3.is_null() {
            serial.writestrs(&["PMM: Test failed (allocation).\n"]);
            return;
        }

        self.free(p2);

        let p4 = self.alloc();
        if p4 != p2 {
            serial.writestrs(&["PMM: Test failed (reallocation).\n"]);
            return;
        }

        self.free(p1);
        self.free(p3);
        self.free(p4);

        serial.writestrs(&["PMM: Test passed.\n"]);
    }
}
