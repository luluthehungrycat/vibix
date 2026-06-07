#include "serial.h"
#include "pmm.h"

/* ---------------------------------------------------------------------------
 * VIBIX kernel entry point — called from kernel_entry.asm.
 *
 * Keeps the boot flow simple: initialise hardware, report alive, hand off.
 * ------------------------------------------------------------------------- */
void kernel_main(void) {
    serial_init();

    serial_puts("\n");
    serial_puts("========================================\n");
    serial_puts("  VIBIX — UNIXoid Kernel\n");
    serial_puts("========================================\n");
    serial_puts("\n");

    serial_puts("VIBIX: Kernel alive!\n");

    /* ── Physical Memory Manager ────────────────────────────────────────── */
    /* For now the PMM manages a fixed range; we tell it about the first 256 MB.
     * Once we parse the Multiboot memory map we can pass the real layout. */
    pmm_init(0x100000, 0x10000000);     /* 1 MB → 256 MB */

    /* Run the built-in self-test */
    pmm_test();

    /* ── Future: PIC/PIT/IDT init will go here ─────────────────────────── */

    serial_puts("VIBIX: Boot sequence complete — entering idle loop.\n");

    /* ── Idle ───────────────────────────────────────────────────────────── */
    for (;;)
        __asm__ volatile ("hlt");
}
