//==============================================================================
// serial.rs — Serial port driver (COM1, 115200 8N1)
//
// Provides raw I/O and a core::fmt::Write implementation so that the
// write!/writeln! macros work for formatted output.
//==============================================================================

use core::fmt;

//--- Register offsets from COM1 base (0x3F8) --------------------------------
const SERIAL_COM1_BASE: u16 = 0x3F8;
const SERIAL_DATA: u16      = 0;
const SERIAL_INTR: u16      = 1;
const SERIAL_FIFO: u16      = 2;
const SERIAL_LCR: u16       = 3;
const SERIAL_MCR: u16       = 4;
const SERIAL_LSR: u16       = 5;

const SERIAL_LSR_THR_EMPTY: u8 = 1 << 5;

//--- Port I/O helpers (inline asm) ------------------------------------------

#[inline]
fn outb(port: u16, val: u8) {
    unsafe { core::arch::asm!("outb %al, %dx", in("al") val, in("dx") port, options(att_syntax)) }
}

#[inline]
fn inb(port: u16) -> u8 {
    let val: u8;
    unsafe { core::arch::asm!("inb %dx, %al", out("al") val, in("dx") port, options(att_syntax)) }
    val
}

//--- SerialPort --------------------------------------------------------------

pub struct SerialPort {
    base: u16,
}

impl SerialPort {
    pub const fn new() -> Self {
        Self { base: SERIAL_COM1_BASE }
    }

    /// Initialise COM1: 115200 baud, 8N1.
    pub fn init(&mut self) {
        outb(self.base + SERIAL_INTR, 0x00);   // disable interrupts
        outb(self.base + SERIAL_LCR,  0x80);   // DLAB on
        outb(self.base + SERIAL_DATA, 0x01);   // divisor low  (115200)
        outb(self.base + SERIAL_INTR, 0x00);   // divisor high
        outb(self.base + SERIAL_LCR,  0x03);   // 8N1
        outb(self.base + SERIAL_FIFO, 0xC7);   // enable FIFO, clear, 14-byte threshold
        outb(self.base + SERIAL_MCR,  0x0B);   // DTR+RTS+OUT2
    }

    /// Write a single character — spin until THR empty.
    pub fn putchar(&mut self, c: char) {
        // Wait for the transmit-holding-register to be empty.
        while (inb(self.base + SERIAL_LSR) & SERIAL_LSR_THR_EMPTY) == 0 {}

        let byte = c as u8;
        outb(self.base + SERIAL_DATA, byte);

        // Carriage-return before newline — many serial consoles need it.
        if c == '\n' {
            while (inb(self.base + SERIAL_LSR) & SERIAL_LSR_THR_EMPTY) == 0 {}
            outb(self.base + SERIAL_DATA, b'\r');
        }
    }

    /// Write one or more string slices without format overhead.
    pub fn writestrs(&mut self, strs: &[&str]) {
        for s in strs {
            for byte in s.bytes() {
                self.putchar(byte as char);
            }
        }
    }
}

//--- core::fmt::Write implementation -----------------------------------------

impl fmt::Write for SerialPort {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            self.putchar(byte as char);
        }
        Ok(())
    }
}
