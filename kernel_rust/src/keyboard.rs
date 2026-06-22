//==============================================================================
// keyboard.rs — PS/2 keyboard driver (scan code set 1)
//
// Translates PS/2 scan codes from IRQ1 to ASCII characters and prints them
// to the serial console for echo.
//==============================================================================

use crate::serial::SerialPort;

//--- Circular buffer for keyboard input -------------------------------------

/// Size of the keyboard input ring buffer.
const BUF_SIZE: usize = 256;

/// Ring buffer for decoded keystrokes.
static mut BUF: [u8; BUF_SIZE] = [0u8; BUF_SIZE];
/// Write index (where the next character goes).
static mut HEAD: usize = 0;
/// Read index (where the next character comes from).
static mut TAIL: usize = 0;

//--- I/O ports ---------------------------------------------------------------

/// PS/2 data port — read scan codes, write commands/data.
const DATA_PORT: u16 = 0x60;
/// PS/2 status / command port.
#[allow(dead_code)]
const STATUS_PORT: u16 = 0x64;

//--- Port I/O ----------------------------------------------------------------

#[inline]
fn inb(port: u16) -> u8 {
    let val: u8;
    unsafe { core::arch::asm!("inb %dx, %al", out("al") val, in("dx") port, options(att_syntax)) }
    val
}

//--- State -------------------------------------------------------------------

/// Whether the left or right shift key is currently held.
static mut SHIFT: bool = false;

//--- Scan code set 1: make → ASCII, unshifted --------------------------------

const SCANCODE_MAKE: [Option<u8>; 0x3B] = {
    let mut t: [Option<u8>; 0x3B] = [None; 0x3B];
    // Row 1: numbers and symbols
    t[0x02] = Some(b'1'); t[0x03] = Some(b'2'); t[0x04] = Some(b'3');
    t[0x05] = Some(b'4'); t[0x06] = Some(b'5'); t[0x07] = Some(b'6');
    t[0x08] = Some(b'7'); t[0x09] = Some(b'8'); t[0x0A] = Some(b'9');
    t[0x0B] = Some(b'0');
    // Row 2: top-alpha
    t[0x10] = Some(b'q'); t[0x11] = Some(b'w'); t[0x12] = Some(b'e');
    t[0x13] = Some(b'r'); t[0x14] = Some(b't'); t[0x15] = Some(b'y');
    t[0x16] = Some(b'u'); t[0x17] = Some(b'i'); t[0x18] = Some(b'o');
    t[0x19] = Some(b'p');
    // Row 3: home row
    t[0x1E] = Some(b'a'); t[0x1F] = Some(b's'); t[0x20] = Some(b'd');
    t[0x21] = Some(b'f'); t[0x22] = Some(b'g'); t[0x23] = Some(b'h');
    t[0x24] = Some(b'j'); t[0x25] = Some(b'k'); t[0x26] = Some(b'l');
    // Row 4: bottom-alpha
    t[0x2C] = Some(b'z'); t[0x2D] = Some(b'x'); t[0x2E] = Some(b'c');
    t[0x2F] = Some(b'v'); t[0x30] = Some(b'b'); t[0x31] = Some(b'n');
    t[0x32] = Some(b'm');
    // Special keys
    t[0x39] = Some(b' '); // space
    t[0x1C] = Some(b'\n'); // enter
    t[0x0E] = Some(b'\x08'); // backspace
    t[0x0F] = Some(b'\t'); // tab
    t
};

//--- Scan code set 1: make → ASCII, shifted ----------------------------------

const SCANCODE_MAKE_SHIFT: [Option<u8>; 0x3B] = {
    let mut t: [Option<u8>; 0x3B] = [None; 0x3B];
    t[0x02] = Some(b'!'); t[0x03] = Some(b'@'); t[0x04] = Some(b'#');
    t[0x05] = Some(b'$'); t[0x06] = Some(b'%'); t[0x07] = Some(b'^');
    t[0x08] = Some(b'&'); t[0x09] = Some(b'*'); t[0x0A] = Some(b'(');
    t[0x0B] = Some(b')');
    // Upper-case letters (shift + alpha)
    t[0x10] = Some(b'Q'); t[0x11] = Some(b'W'); t[0x12] = Some(b'E');
    t[0x13] = Some(b'R'); t[0x14] = Some(b'T'); t[0x15] = Some(b'Y');
    t[0x16] = Some(b'U'); t[0x17] = Some(b'I'); t[0x18] = Some(b'O');
    t[0x19] = Some(b'P');
    t[0x1E] = Some(b'A'); t[0x1F] = Some(b'S'); t[0x20] = Some(b'D');
    t[0x21] = Some(b'F'); t[0x22] = Some(b'G'); t[0x23] = Some(b'H');
    t[0x24] = Some(b'J'); t[0x25] = Some(b'K'); t[0x26] = Some(b'L');
    t[0x2C] = Some(b'Z'); t[0x2D] = Some(b'X'); t[0x2E] = Some(b'C');
    t[0x2F] = Some(b'V'); t[0x30] = Some(b'B'); t[0x31] = Some(b'N');
    t[0x32] = Some(b'M');
    t
};

//--- Public API --------------------------------------------------------------

/// Called from the IRQ dispatcher on each IRQ1 (keyboard interrupt).
pub fn handle_keyboard() {
    // Read the scan code from the PS/2 data port.
    let scancode = inb(DATA_PORT);

    // Ignore extended prefixes (0xE0, 0xE1) — not implemented.
    if scancode == 0xE0 || scancode == 0xE1 {
        return;
    }

    if scancode == 0x2A || scancode == 0x36 {
        // Shift make — set the shift flag.
        unsafe { SHIFT = true; }
        return;
    }

    if scancode == 0xAA || scancode == 0xB6 {
        // Shift break — clear the shift flag.
        unsafe { SHIFT = false; }
        return;
    }

    // Process only make codes (break codes have bit 7 set).
    if scancode & 0x80 != 0 {
        return; // break code — ignore
    }

    // Skip weird scan codes above our table bounds.
    if scancode as usize >= SCANCODE_MAKE.len() {
        return;
    }

    let shifted = unsafe { SHIFT };
    let table = if shifted { &SCANCODE_MAKE_SHIFT } else { &SCANCODE_MAKE };
    if let Some(byte) = table[scancode as usize] {
        // Echo to serial console.
        let mut serial = SerialPort::new();
        serial.putchar(byte as char);

        // Push into the circular keyboard buffer (non-blocking, drops on full).
        unsafe {
            let next_head = (HEAD + 1) % BUF_SIZE;
            if next_head != TAIL {
                BUF[HEAD] = byte;
                HEAD = next_head;
            }
        }
    }
}

/// Copy buffered keyboard input into `buf`, returning the number of bytes read.
///
/// This is a non-blocking read — returns 0 immediately if no data is available.
pub fn read(buf: &mut [u8]) -> usize {
    let mut count = 0usize;
    unsafe {
        while HEAD != TAIL && count < buf.len() {
            buf[count] = BUF[TAIL];
            TAIL = (TAIL + 1) % BUF_SIZE;
            count += 1;
        }
    }
    count
}
