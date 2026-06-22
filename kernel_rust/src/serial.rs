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

/// Bit 0 of LSR: Data Ready (receiver has data).
const SERIAL_LSR_DATA_READY: u8 = 0x01;

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

    /// Max iterations to spin-wait for THR empty before giving up.
    const PUTCHAR_TIMEOUT: u32 = 1_000_000;

    /// Write a single character — spin until THR empty (with timeout).
    pub fn putchar(&mut self, c: char) {
        let byte = c as u8;

        // Wait for the transmit-holding-register to be empty.
        let mut timeout = Self::PUTCHAR_TIMEOUT;
        while (inb(self.base + SERIAL_LSR) & SERIAL_LSR_THR_EMPTY) == 0 {
            timeout -= 1;
            if timeout == 0 {
                return;  // give up — don't hang
            }
        }
        outb(self.base + SERIAL_DATA, byte);

        // Carriage-return before newline — many serial consoles need it.
        if c == '\n' {
            let mut timeout = Self::PUTCHAR_TIMEOUT;
            while (inb(self.base + SERIAL_LSR) & SERIAL_LSR_THR_EMPTY) == 0 {
                timeout -= 1;
                if timeout == 0 {
                    return;
                }
            }
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

    /// Try to read a single byte.  Returns `None` if no data is available
    /// (non-blocking).
    pub fn getchar(&self) -> Option<u8> {
        if inb(self.base + SERIAL_LSR) & SERIAL_LSR_DATA_READY != 0 {
            Some(inb(self.base + SERIAL_DATA))
        } else {
            None
        }
    }

    /// Non-blocking read: copy available bytes from the serial receive buffer
    /// into `buf`.  Returns the number of bytes read (0 if empty).
    pub fn read(&self, buf: &mut [u8]) -> usize {
        let mut count = 0usize;
        while count < buf.len() {
            match self.getchar() {
                Some(byte) => {
                    buf[count] = byte;
                    count += 1;
                }
                None => break,
            }
        }
        count
    }

    /// Blocking read: waits for at least one byte, then drains whatever else
    /// is available into `buf`.  Returns the number of bytes read (> 0).
    pub fn read_blocking(&self, buf: &mut [u8]) -> usize {
        // Spin until at least one byte is available.
        while (inb(self.base + SERIAL_LSR) & SERIAL_LSR_DATA_READY) == 0 {
            core::hint::spin_loop();
        }
        let mut count = 0usize;
        // Read the first byte (we know it's ready).
        buf[count] = inb(self.base + SERIAL_DATA);
        count += 1;
        // Drain any additional bytes that are available now.
        while count < buf.len() {
            if (inb(self.base + SERIAL_LSR) & SERIAL_LSR_DATA_READY) != 0 {
                buf[count] = inb(self.base + SERIAL_DATA);
                count += 1;
            } else {
                break;
            }
        }
        count
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
