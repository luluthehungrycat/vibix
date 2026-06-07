#include "pmm.h"
#include "serial.h"
#include <stdint.h>

/* ---------------------------------------------------------------------------
 * Physical Memory Manager — bitmap allocator.
 *
 * Each bit represents one 4 KB page.  1 = allocated, 0 = free.
 * The bitmap lives in statically-allocated BSS (zeroed at boot by the
 * Multiboot loader).  It can track PMM_MAX_PAGES (64 K  = 256 MB).
 * ------------------------------------------------------------------------- */

#define BITMAP_SIZE  (PMM_MAX_PAGES / 8)
static uint8_t pmm_bitmap[BITMAP_SIZE];

/* Forward declaration needed because itoa() is called before it's defined. */
static char *itoa(size_t num);

/* ---------------------------------------------------------------------------
 * Initialise the PMM over a given physical memory region.
 *
 * Reserves the first 4 pages (for the bitmap itself and early kernel code)
 * and marks everything else as free until pmm_init_region() is called.
 * ------------------------------------------------------------------------- */
void pmm_init(uintptr_t memory_start, uintptr_t memory_end) {
    size_t total_pages = (memory_end - memory_start) / PMM_PAGE_SIZE;
    if (total_pages > PMM_MAX_PAGES)
        total_pages = PMM_MAX_PAGES;

    /* Zero the bitmap (BSS wasn't zeroed by the bootloader for the embedded
     * 64-bit binary — we do it here instead). */
    for (size_t i = 0; i < BITMAP_SIZE; i++)
        pmm_bitmap[i] = 0;

    /* Mark the first 4 pages as used (bitmap + early kernel/bootstrap). */
    for (size_t i = 0; i < 4; i++)
        pmm_bitmap[i / 8] |= (uint8_t)(1 << (i % 8));

    serial_puts("PMM: Initialized with ");
    serial_puts(itoa(total_pages));
    serial_puts(" pages (max ");
    serial_puts(itoa(PMM_MAX_PAGES));
    serial_puts(").\n");
}

/* ---------------------------------------------------------------------------
 * Allocate a single 4 KB page.  Returns NULL when exhausted.
 * ------------------------------------------------------------------------- */
void *pmm_alloc(void) {
    for (size_t i = 0; i < PMM_MAX_PAGES; i++) {
        if (!(pmm_bitmap[i / 8] & (uint8_t)(1 << (i % 8)))) {
            pmm_bitmap[i / 8] |= (uint8_t)(1 << (i % 8));
            return (void *)(i * PMM_PAGE_SIZE);
        }
    }
    serial_puts("PMM: Out of memory!\n");
    return (void *)0;  /* NULL in freestanding */
}

/* ---------------------------------------------------------------------------
 * Free a previously-allocated page.
 * ------------------------------------------------------------------------- */
void pmm_free(void *ptr) {
    uintptr_t addr = (uintptr_t)ptr;
    size_t page = addr / PMM_PAGE_SIZE;
    if (page >= PMM_MAX_PAGES) {
        serial_puts("PMM: Invalid free address!\n");
        return;
    }
    pmm_bitmap[page / 8] &= (uint8_t)~(1 << (page % 8));
}

/* ---------------------------------------------------------------------------
 * Mark a region of physical memory as usable (free).
 * ------------------------------------------------------------------------- */
void pmm_init_region(uintptr_t base, size_t size) {
    size_t start_page = base / PMM_PAGE_SIZE;
    size_t end_page   = (base + size - 1) / PMM_PAGE_SIZE;
    if (end_page >= PMM_MAX_PAGES)
        end_page = PMM_MAX_PAGES - 1;

    for (size_t i = start_page; i <= end_page; i++)
        pmm_bitmap[i / 8] &= (uint8_t)~(1 << (i % 8));
}

/* ---------------------------------------------------------------------------
 * Quick self-test: allocate, free, reallocate, verify reuse.
 * ------------------------------------------------------------------------- */
void pmm_test(void) {
    void *p1 = pmm_alloc();
    void *p2 = pmm_alloc();
    void *p3 = pmm_alloc();

    if (!p1 || !p2 || !p3) {
        serial_puts("PMM: Test failed (allocation).\n");
        return;
    }

    pmm_free(p2);

    void *p4 = pmm_alloc();
    if (p4 != p2) {
        serial_puts("PMM: Test failed (reallocation — expected ");
        serial_puts(itoa((size_t)p2));
        serial_puts(" got ");
        serial_puts(itoa((size_t)p4));
        serial_puts(").\n");
        return;
    }

    pmm_free(p1);
    pmm_free(p3);
    pmm_free(p4);

    serial_puts("PMM: Test passed.\n");
}

/* ---------------------------------------------------------------------------
 * Minimal integer-to-string — no libc required.
 * ------------------------------------------------------------------------- */
static char *itoa(size_t num) {
    static char buf[20];
    int i = 0;

    if (num == 0) {
        buf[i++] = '0';
    } else {
        while (num > 0) {
            buf[i++] = '0' + (num % 10);
            num /= 10;
        }
    }
    buf[i] = '\0';

    /* Reverse in place. */
    for (int j = 0; j < i / 2; j++) {
        char tmp   = buf[j];
        buf[j]     = buf[i - 1 - j];
        buf[i - 1 - j] = tmp;
    }
    return buf;
}
