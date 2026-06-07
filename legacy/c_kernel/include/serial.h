#ifndef SERIAL_H
#define SERIAL_H

/** Serial (COM1) port addresses. */
#define SERIAL_COM1_BASE  0x3F8

#define SERIAL_DATA       0
#define SERIAL_INTR       1
#define SERIAL_FIFO       2
#define SERIAL_LCR        3
#define SERIAL_MCR        4
#define SERIAL_LSR        5

/* Line status register bits */
#define SERIAL_LSR_THR_EMPTY  (1 << 5)

void serial_init(void);
void serial_putchar(char c);
void serial_puts(const char *str);

#endif /* SERIAL_H */
