//==============================================================================
// pit.rs — Intel 8253 Programmable Interval Timer (PIT) driver
//
// Programs PIT channel 0 to generate IRQ0 at approximately 100 Hz.
// Provides a monotonically increasing tick counter.
//==============================================================================

use crate::serial::SerialPort;

//--- PIT I/O ports -----------------------------------------------------------

/// PIT channel 0 data port (connected to IRQ0).
const PIT_CH0: u16 = 0x40;
/// PIT command / mode register.
const PIT_CMD: u16 = 0x43;

//--- Constants ---------------------------------------------------------------

/// PIT base frequency (1.1931816... MHz).
const PIT_BASE_FREQ: u64 = 1_193_182;
/// Desired IRQ0 frequency.
const TARGET_FREQ: u64 = 100;  // 100 Hz → 10 ms intervals
/// Divisor loaded into channel 0.
const PIT_DIVISOR: u16 = (PIT_BASE_FREQ / TARGET_FREQ) as u16;

//--- Command byte (program channel 0) ---------------------------------------
//
// Bits 7-6: channel select  → 00 (channel 0)
// Bits 5-4: access mode     → 11 (lobyte then hibyte)
// Bits 3-1: operating mode  → 011 (mode 3 — square wave)
// Bit 0:    BCD/binary      → 0  (16-bit binary)
//
// => 0b0011_0110 = 0x36

const PIT_CMD_VALUE: u8 = 0x36;

//--- State -------------------------------------------------------------------

/// Monotonically increasing tick count.  Incremented on each IRQ0.
static mut PIT_TICKS: u64 = 0;

//--- Port I/O ----------------------------------------------------------------

#[inline]
fn outb(port: u16, val: u8) {
    unsafe { core::arch::asm!("outb %al, %dx", in("al") val, in("dx") port, options(att_syntax)) }
}

//--- Public API --------------------------------------------------------------

/// Initialise PIT channel 0 to generate IRQ0 at ~100 Hz.
pub fn init() {
    // outb() is unsafe; wrapping block OK but let the function bodies carry it.
    outb(PIT_CMD, PIT_CMD_VALUE);
    outb(PIT_CH0, (PIT_DIVISOR & 0xFF) as u8);  // low byte
    outb(PIT_CH0, (PIT_DIVISOR >> 8) as u8);     // high byte
}

/// Called by the interrupt dispatcher on each IRQ0.
pub fn tick() {
    unsafe {
        PIT_TICKS += 1;
        let ticks = PIT_TICKS;

        // Print a heartbeat every 100 ticks (≈ 1 second).
        if ticks % 100 == 0 {
            let mut serial = SerialPort::new();
            serial.writestrs(&["VIBIX: PIT tick #"]);

            // Manual decimal formatting (no_std).
            let mut n = ticks;
            let mut digits = [0u8; 20];
            let mut i = digits.len();
            while n > 0 {
                i -= 1;
                digits[i] = b'0' + (n % 10) as u8;
                n /= 10;
            }
            if i == digits.len() {
                // ticks == 0
                i -= 1;
                digits[i] = b'0';
            }
            // Safety: digits[i..] contains only ASCII digits.
            let s = core::str::from_utf8_unchecked(&digits[i..]);
            serial.writestrs(&[s, "\n"]);
        }
    }
}

/// Return the current tick count.
#[allow(dead_code)]
pub fn get_ticks() -> u64 {
    unsafe { PIT_TICKS }
}
