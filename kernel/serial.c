#include "serial.h"
#include <stdint.h>

/* ---------------------------------------------------------------------------
 * Inline I/O helpers (NECESSARY in freestanding — no libc wrappers)
 * ------------------------------------------------------------------------- */
static inline void outb(uint16_t port, uint8_t val) {
    __asm__ volatile ("outb %0, %1" : : "a"(val), "Nd"(port));
}

static inline uint8_t inb(uint16_t port) {
    uint8_t ret;
    __asm__ volatile ("inb %1, %0" : "=a"(ret) : "Nd"(port));
    return ret;
}

/* ---------------------------------------------------------------------------
 * Initialise COM1: 115200 baud, 8N1
 * ------------------------------------------------------------------------- */
void serial_init(void) {
    outb(SERIAL_COM1_BASE + SERIAL_INTR, 0x00);   // disable interrupts
    outb(SERIAL_COM1_BASE + SERIAL_LCR,   0x80);   // DLAB on
    outb(SERIAL_COM1_BASE + SERIAL_DATA,  0x01);   // divisor low  (115200)
    outb(SERIAL_COM1_BASE + SERIAL_INTR,  0x00);   // divisor high
    outb(SERIAL_COM1_BASE + SERIAL_LCR,   0x03);   // 8N1
    outb(SERIAL_COM1_BASE + SERIAL_FIFO,  0xC7);   // enable FIFO, clear, 14-byte threshold
    outb(SERIAL_COM1_BASE + SERIAL_MCR,   0x0B);   // DTR+RTS+OUT2
}

/* ---------------------------------------------------------------------------
 * Write a single character — spin until THR empty.
 * ------------------------------------------------------------------------- */
void serial_putchar(char c) {
    /* Wait for the transmit-holding-register to be empty. */
    while (!(inb(SERIAL_COM1_BASE + SERIAL_LSR) & SERIAL_LSR_THR_EMPTY))
        ;
    outb(SERIAL_COM1_BASE + SERIAL_DATA, (uint8_t)c);

    /* Carriage-return before newline — many serial consoles need it. */
    if (c == '\n')
        serial_putchar('\r');
}

/* ---------------------------------------------------------------------------
 * Write a null-terminated string.
 * ------------------------------------------------------------------------- */
void serial_puts(const char *str) {
    for (; *str != '\0'; str++)
        serial_putchar(*str);
}
