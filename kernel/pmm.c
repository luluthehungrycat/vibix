#include "pmm.h"
#include "serial.h"
#include <stdint.h>
#include <string.h>

static uint8_t pmm_bitmap[PMM_MAX_PAGES / 8] = {0};

void pmm_init(uintptr_t memory_start, uintptr_t memory_end) {
    size_t total_pages = (memory_end - memory_start) / PMM_PAGE_SIZE;
    if (total_pages > PMM_MAX_PAGES) {
        total_pages = PMM_MAX_PAGES;
    }
    // Reserve first 4 pages (for bitmap and early kernel)
    for (size_t i = 0; i < 4; i++) {
        pmm_bitmap[i / 8] |= (1 << (i % 8));
    }
    serial_puts("PMM: Initialized with ");
    serial_puts(itoa(total_pages));
    serial_puts(" pages.\n");
}

void* pmm_alloc() {
    for (size_t i = 0; i < PMM_MAX_PAGES; i++) {
        if (!(pmm_bitmap[i / 8] & (1 << (i % 8)))) {
            pmm_bitmap[i / 8] |= (1 << (i % 8));
            return (void*)(i * PMM_PAGE_SIZE);
        }
    }
    serial_puts("PMM: Out of memory!\n");
    return NULL;
}

void pmm_free(void* ptr) {
    uintptr_t addr = (uintptr_t)ptr;
    size_t page = addr / PMM_PAGE_SIZE;
    if (page >= PMM_MAX_PAGES) {
        serial_puts("PMM: Invalid free address!\n");
        return;
    }
    pmm_bitmap[page / 8] &= ~(1 << (page % 8));
}

void pmm_init_region(uintptr_t base, size_t size) {
    size_t start_page = base / PMM_PAGE_SIZE;
    size_t end_page = (base + size - 1) / PMM_PAGE_SIZE;
    if (end_page >= PMM_MAX_PAGES) {
        end_page = PMM_MAX_PAGES - 1;
    }
    for (size_t i = start_page; i <= end_page; i++) {
        pmm_bitmap[i / 8] &= ~(1 << (i % 8));  // Mark as free
    }
}

// Helper: Convert integer to string (for serial_puts)
char* itoa(int num) {
    static char buf[20];
    int i = 0;
    if (num == 0) {
        buf[i++] = '0';
    } else {
        while (num > 0) {
            buf[i++] = '0' + (num % 10);
            num /= 10;
        }
        for (int j = 0; j < i / 2; j++) {
            char tmp = buf[j];
            buf[j] = buf[i - 1 - j];
            buf[i - 1 - j] = tmp;
        }
    }
    buf[i] = '\0';
    return buf;
}

void pmm_test() {
    void* p1 = pmm_alloc();
    void* p2 = pmm_alloc();
    void* p3 = pmm_alloc();
    if (!p1 || !p2 || !p3) {
        serial_puts("PMM: Test failed (allocation).\n");
        return;
    }
    pmm_free(p2);
    void* p4 = pmm_alloc();
    if (p4 != p2) {
        serial_puts("PMM: Test failed (reallocation).\n");
        return;
    }
    pmm_free(p1);
    pmm_free(p3);
    pmm_free(p4);
    serial_puts("PMM: Test passed.\n");
}
