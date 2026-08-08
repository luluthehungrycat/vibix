//==============================================================================
// vfs/tty.rs — ANSI Terminal Driver
//
// Provides a line-discipline layer over the keyboard (input) and serial port
// (output). In canonical mode (the default) characters are accumulated into
// a line buffer, with support for backspace, Ctrl+C line discard, and local
// echo. Completed lines are moved to a ring buffer for read().
//
// The keyboard IRQ handler no longer echoes — all echo is delegated here so
// that backspace and other line-discipline processing works correctly.
//==============================================================================

use crate::vfs::*;

//==============================================================================
// Constants
//==============================================================================

/// Ring buffer for completed lines (bytes ready for read()).
const BUF_SIZE: usize = 4096;

/// Maximum length of a single line being built.
const LINE_MAX: usize = 255;

//==============================================================================
// Tty line discipline state
//==============================================================================

pub struct Tty {
    /// Completed-data ring buffer — bytes that tty_read() returns.
    buf: [u8; BUF_SIZE],
    head: usize,
    tail: usize,

    /// Current partial line being built (canonical mode).
    line: [u8; LINE_MAX],
    line_len: usize,

    /// Echo input characters back to serial.
    echo: bool,
    /// Canonical mode — line buffered vs raw.
    canonical: bool,

    /// PID of process blocked waiting for TTY input (0 = none).
    waiting_pid: u64,
}

//==============================================================================
// Singleton — one Tty for the whole system
//==============================================================================

/// The single kernel Tty instance.  Lives forever; accessed only with
/// interrupts disabled (same pattern as PROCESS_TABLE).
pub static mut TTY: Tty = Tty::new();

impl Tty {
    pub const fn new() -> Self {
        Tty {
            buf: [0; BUF_SIZE],
            head: 0,
            tail: 0,
            line: [0; LINE_MAX],
            line_len: 0,
            echo: true,
            canonical: true,
            waiting_pid: 0,
        }
    }

    //--------------------------------------------------------------------------
    // Internal: line discipline
    //--------------------------------------------------------------------------

    /// Pull all available input from keyboard and serial, feeding it through
    /// the line discipline.
    fn process_input(&mut self) {
        // 1. Drain keyboard buffer
        let mut scratch = [0u8; 64];
        let n = crate::keyboard::read(&mut scratch);
        for i in 0..n {
            self.process_byte(scratch[i]);
        }

        // 2. Drain serial receiver buffer
        let serial = crate::serial::SerialPort::new();
        let n2 = serial.read(&mut scratch);
        if cfg!(feature = "debug") && n2 != 0 {
            debug_marker(
                "serial",
                crate::process::current_pid(),
                self.waiting_pid,
                n2 as u64,
            );
        }
        for i in 0..n2 {
            self.process_byte(scratch[i]);
        }
    }

    /// Process a single byte through the line discipline.
    fn process_byte(&mut self, c: u8) {
        if !self.canonical {
            // Raw mode: push directly to completed buffer, no echo
            self.push_byte(c);
            return;
        }

        match c {
            0x08 | 0x7F => self.handle_backspace(),
            0x03 => self.handle_ctrlc(),
            b'\r' | b'\n' => self.handle_enter(),
            0x09 => self.handle_tab(),
            0x1b => {
                // Escape — ignored for now (future: ANSI escape sequences)
            }
            _ if c >= 0x20 => self.handle_char(c),
            _ => {} // Discard other control characters
        }
    }

    fn handle_backspace(&mut self) {
        if self.line_len > 0 {
            self.line_len -= 1;
            if self.echo {
                // Move cursor back, overwrite with space, move cursor back.
                serial_write(&[0x08, b' ', 0x08]);
            }
        }
    }

    fn handle_ctrlc(&mut self) {
        let reader_pid = self.waiting_pid;
        if cfg!(feature = "debug") {
            debug_marker(
                "ctrlc",
                reader_pid,
                crate::process::current_pid(),
                self.line_len as u64,
            );
        }
        self.line_len = 0;
        if self.echo {
            serial_write(b"^C\r\n");
        }

        // The scheduler polls input while the reader is blocked, so
        // current_pid() is commonly the idle process here.  Only the live,
        // blocked owner recorded by read() may receive terminal SIGINT.
        if reader_pid != 0 {
            let delivered = if let Some(proc) = crate::process::process_mut_if(reader_pid) {
                if proc.state == crate::process::ProcessState::Blocked {
                    proc.sig_pending |= 1 << crate::signal::SIGINT;
                    true
                } else {
                    false
                }
            } else {
                false
            };
            if !delivered {
                // Do not let a stale or no-longer-blocked owner affect an
                // unrelated process when push_byte() publishes the newline.
                self.waiting_pid = 0;
            }
        }

        // Push a newline through the normal completed-input path.  This
        // preserves the existing wakeup and scheduling behavior.
        self.push_byte(b'\n');
    }

    fn handle_enter(&mut self) {
        if self.echo {
            serial_write(b"\r\n");
        }
        // Move completed line to ring buffer
        for i in 0..self.line_len {
            self.push_byte(self.line[i]);
        }
        self.push_byte(b'\n');
        self.line_len = 0;
    }

    fn handle_tab(&mut self) {
        if self.echo {
            serial_write(b"\t");
        }
        if self.line_len < LINE_MAX {
            self.line[self.line_len] = b'\t';
            self.line_len += 1;
        }
    }

    fn handle_char(&mut self, c: u8) {
        if self.line_len < LINE_MAX {
            self.line[self.line_len] = c;
            self.line_len += 1;
            if self.echo {
                serial_write(&[c]);
            }
        }
    }

    //--------------------------------------------------------------------------
    // Ring buffer helpers
    //--------------------------------------------------------------------------

    /// Push one byte into the completed-data ring buffer.
    /// Drops the byte silently if the buffer is full.
    #[inline]
    fn push_byte(&mut self, b: u8) {
        let next = (self.head + 1) % BUF_SIZE;
        if next != self.tail {
            self.buf[self.head] = b;
            self.head = next;
        }
        // Wake any process blocked waiting for TTY input
        if self.waiting_pid != 0 {
            let pid = self.waiting_pid;
            self.waiting_pid = 0;
            if let Some(proc) = crate::process::process_mut_if(pid) {
                if cfg!(feature = "debug") {
                    debug_marker("wake", pid, proc.state as u64, self.head as u64);
                }
                if proc.state == crate::process::ProcessState::Blocked {
                    proc.state = crate::process::ProcessState::Ready;
                    unsafe {
                        core::ptr::write_volatile(&raw mut crate::process::should_schedule, 1);
                    }
                }
            }
        }
    }

    //--------------------------------------------------------------------------
    // Public interface (called from VnodeOps)
    //--------------------------------------------------------------------------

    /// Poll keyboard and serial input from the scheduler tick.
    ///
    /// A process blocked in read() cannot poll its own input device. The timer
    /// calls this entry point so serial input can reach push_byte(), which
    /// wakes the process recorded in waiting_pid.
    pub fn poll_input() {
        let tty = unsafe { &mut *core::ptr::addr_of_mut!(TTY) };
        tty.process_input();
    }

    /// Read from the completed-lines buffer.
    ///
    /// First processes any pending keyboard/serial input through the line
    /// discipline.  Returns 0 if no complete line is available yet
    /// (non-blocking).
    pub fn read(&mut self, buf: &mut [u8]) -> usize {
        self.process_input();

        // If data available, return it
        if self.tail != self.head {
            let mut count = 0;
            while self.tail != self.head && count < buf.len() {
                buf[count] = self.buf[self.tail];
                self.tail = (self.tail + 1) % BUF_SIZE;
                count += 1;
            }
            if cfg!(feature = "debug") {
                debug_marker(
                    "read",
                    crate::process::current_pid(),
                    count as u64,
                    self.waiting_pid,
                );
            }
            return count;
        }

        // No data — block the current process until input arrives.
        // The process will be woken by push_byte() when keyboard data arrives.
        let pid = crate::process::current_pid();
        if pid != 0 {
            let proc = crate::process::process_mut(pid);
            proc.state = crate::process::ProcessState::Blocked;
            self.waiting_pid = pid;
            if cfg!(feature = "debug") {
                debug_marker("block", pid, self.tail as u64, self.head as u64);
            }
            unsafe {
                core::ptr::write_volatile(&raw mut crate::process::should_schedule, 1);
            }
        }
        if cfg!(feature = "debug") {
            debug_marker("read", crate::process::current_pid(), 0, self.waiting_pid);
        }
        0
    }

    /// Write to serial output.
    pub fn write(&mut self, buf: &[u8]) -> usize {
        serial_write(buf);
        buf.len()
    }
}

//------------------------------------------------------------------------------
// I/O helper — write bytes to serial without creating a Tty dependency
//------------------------------------------------------------------------------

fn debug_marker(event: &str, a: u64, b: u64, c: u64) {
    if !cfg!(feature = "debug") {
        return;
    }
    use core::fmt::Write;
    let mut serial = crate::serial::SerialPort::new();
    serial.init();
    let _ = write!(
        serial,
        "DBG tty: t={} event={} a={} b={} c={}\n",
        crate::pit::get_ticks(),
        event,
        a,
        b,
        c
    );
}

fn serial_write(buf: &[u8]) {
    let mut serial = crate::serial::SerialPort::new();
    for &b in buf {
        serial.putchar(b as char);
    }
    // Note: no init() — COM1 was initialised during early boot.
}

//==============================================================================
// VnodeOps table
//==============================================================================

pub static TTY_OPS: VnodeOps = VnodeOps {
    open: Some(tty_open as VnOpen),
    close: Some(tty_close as VnClose),
    read: Some(tty_read as VnRead),
    write: Some(tty_write as VnWrite),
    lseek: None,
    readdir: None,
    ioctl: Some(tty_ioctl as VnIoctl),
};

//==============================================================================
// Vnode operation implementations
//==============================================================================

unsafe fn tty_open(_vnode: *mut Vnode, _flags: u32, _mode: u32) -> i32 {
    0
}
unsafe fn tty_close(_vnode: *mut Vnode) -> i32 {
    0
}

unsafe fn tty_read(_vnode: *mut Vnode, buf: *mut u8, len: usize, _offset: &mut u64) -> isize {
    let tty = &mut *core::ptr::addr_of_mut!(TTY);
    let slice = core::slice::from_raw_parts_mut(buf, len);
    tty.read(slice) as isize
}

unsafe fn tty_write(_vnode: *mut Vnode, buf: *const u8, len: usize, _offset: &mut u64) -> isize {
    let tty = &mut *core::ptr::addr_of_mut!(TTY);
    let slice = core::slice::from_raw_parts(buf, len);
    tty.write(slice) as isize
}

pub unsafe fn tty_ioctl(_vnode: *mut Vnode, request: u64, arg: u64) -> i32 {
    if arg == 0 {
        return -EFAULT;
    }
    match request as u32 {
        // TIOCGWINSZ (0x5413): Get window size — return 80x25
        0x5413 => {
            let winsize: [u16; 4] = [25, 80, 0, 0];
            core::ptr::copy_nonoverlapping(winsize.as_ptr(), arg as *mut u16, 4);
            0
        }
        // TCGETS (0x5401): Get terminal attributes
        0x5401 => {
            let mut termios = [0u8; 60];
            // c_cflag at offset 8: B9600 | CS8 | CREAD | CLOCAL = 0xBFD
            termios[8..12].copy_from_slice(&[0xFD, 0x0B, 0x00, 0x00]);
            // c_lflag at offset 12: ICANON | ECHO | ISIG = 0x3CB
            termios[12..16].copy_from_slice(&[0xCB, 0x03, 0x00, 0x00]);
            core::ptr::copy_nonoverlapping(termios.as_ptr(), arg as *mut u8, 60);
            0
        }
        // TCSETS (0x5402): Set terminal attributes — no-op
        0x5402 => 0,
        _ => -ENOTTY,
    }
}
