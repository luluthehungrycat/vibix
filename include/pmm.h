#ifndef PMM_H
#define PMM_H

#include <stdint.h>
#include <stddef.h>

#define PMM_PAGE_SIZE 4096
#define PMM_MAX_PAGES 0x10000  // 256MB / 4KB

extern void pmm_init(uintptr_t memory_start, uintptr_t memory_end);
extern void* pmm_alloc();
extern void pmm_free(void* ptr);
extern void pmm_init_region(uintptr_t base, size_t size);
extern void pmm_test();

#endif
