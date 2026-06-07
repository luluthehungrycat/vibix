#ifndef PMM_H
#define PMM_H

#include <stdint.h>
#include <stddef.h>

#define PMM_PAGE_SIZE  4096
#define PMM_MAX_PAGES  0x10000       /* 64 K pages → 256 MB */

void  pmm_init(uintptr_t memory_start, uintptr_t memory_end);
void *pmm_alloc(void);
void  pmm_free(void *ptr);
void  pmm_init_region(uintptr_t base, size_t size);
void  pmm_test(void);

#endif /* PMM_H */
