//==============================================================================
// interrupts.rs — IDT setup, PIC remapping, and exception handling
//
// Provides:
//   - IDT (Interrupt Descriptor Table) with 256 entries
//   - ISR entry points for CPU exceptions (0–31)
//   - PIC 8259A remapping to avoid reserved vectors
//   - Default handler that prints exception info and halts
//==============================================================================

use core::fmt::Write;
use crate::serial::SerialPort;

//------------------------------------------------------------------------------
// IDT entry — 16 bytes per entry
//------------------------------------------------------------------------------

/// A single IDT entry. Layout matches the x86_64 IDT entry format.
#[derive(Clone, Copy)]
#[repr(C, packed)]
pub struct IdtEntry {
    offset_low: u16,   // Bits 0-15 of handler address
    selector: u16,     // Code segment selector (0x08 for kernel)
    ist: u8,           // Interrupt Stack Table offset (0 = disabled)
    flags: u8,         // Type, DPL, Present
    offset_mid: u16,   // Bits 16-31 of handler address
    offset_high: u32,  // Bits 32-63 of handler address
    reserved: u32,
}

impl IdtEntry {
    /// Create an empty (disabled) IDT entry.
    pub const fn missing() -> Self {
        Self {
            offset_low: 0,
            selector: 0,
            ist: 0,
            flags: 0,
            offset_mid: 0,
            offset_high: 0,
            reserved: 0,
        }
    }

    /// Create an interrupt gate entry for the given handler address.
    ///
    /// `selector` is the code segment (typically 0x08).
    /// `dpl` is the descriptor privilege level (0 = kernel, 3 = user).
    pub fn set_handler(&mut self, handler: u64, selector: u16, dpl: u8) {
        self.offset_low = handler as u16;
        self.selector = selector;
        self.ist = 0;
        self.flags = 0x8E | ((dpl & 3) << 5);  // present, ring dpl, interrupt gate (0xE)
        self.offset_mid = (handler >> 16) as u16;
        self.offset_high = (handler >> 32) as u32;
        self.reserved = 0;
    }
}

//------------------------------------------------------------------------------
// IDT — 256-entry table
//------------------------------------------------------------------------------

/// The full Interrupt Descriptor Table (256 entries × 16 bytes = 4096 bytes).
#[repr(C, align(16))]
pub struct Idt {
    entries: [IdtEntry; 256],
}

impl Idt {
    pub const fn new() -> Self {
        Self { entries: [IdtEntry::missing(); 256] }
    }

    pub fn set(&mut self, index: usize, handler: u64, selector: u16, dpl: u8) {
        if index < 256 {
            self.entries[index].set_handler(handler, selector, dpl);
        }
    }
}

//------------------------------------------------------------------------------
// IDT descriptor (for lidt instruction)
//------------------------------------------------------------------------------

/// 10-byte descriptor passed to the `lidt` instruction.
#[repr(C, packed)]
struct IdtDescriptor {
    limit: u16,
    base: u64,
}

//------------------------------------------------------------------------------
// External ISR entry points (from interrupts.asm)
//------------------------------------------------------------------------------

extern "C" {
    fn isr0();
    fn isr1();
    fn isr2();
    fn isr3();
    fn isr4();
    fn isr5();
    fn isr6();
    fn isr7();
    fn isr8();
    fn isr9();
    fn isr10();
    fn isr11();
    fn isr12();
    fn isr13();
    fn isr14();
    fn isr15();
    fn isr16();
    fn isr17();
    fn isr18();
    fn isr19();
    fn isr20();
    fn isr21();
    fn isr22();
    fn isr23();
    fn isr24();
    fn isr25();
    fn isr26();
    fn isr27();
    fn isr28();
    fn isr29();
    fn isr30();
    fn isr31();
}

/// Array of ISR entry points, indexed by interrupt number.
static ISR_STUBS: [unsafe extern "C" fn(); 32] = [
    isr0,  isr1,  isr2,  isr3,  isr4,  isr5,  isr6,  isr7,
    isr8,  isr9,  isr10, isr11, isr12, isr13, isr14, isr15,
    isr16, isr17, isr18, isr19, isr20, isr21, isr22, isr23,
    isr24, isr25, isr26, isr27, isr28, isr29, isr30, isr31,
];

//------------------------------------------------------------------------------
// Register frame (matches stack layout in interrupts.asm)
//------------------------------------------------------------------------------

/// Saved register state, matching the push order in `isr_common` in
/// `interrupts.asm`.  Fields ordered from low to high addresses.
#[repr(C)]
#[derive(Debug)]
pub struct SavedRegisters {
    pub r15: u64, pub r14: u64, pub r13: u64, pub r12: u64,
    pub r11: u64, pub r10: u64, pub r9: u64, pub r8: u64,
    pub rdi: u64, pub rsi: u64, pub rbp: u64, pub rbx: u64,
    pub rdx: u64, pub rcx: u64, pub rax: u64,
}

/// Complete interrupt frame, including CPU-pushed state.
#[repr(C)]
#[derive(Debug)]
pub struct InterruptFrame {
    pub regs: SavedRegisters,
    pub int_no: u64,
    pub err_code: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
}

//------------------------------------------------------------------------------
// PIC 8259A ports and constants
//------------------------------------------------------------------------------

const PIC1_COMMAND: u16 = 0x20;
const PIC1_DATA:    u16 = 0x21;
const PIC2_COMMAND: u16 = 0xA0;
const PIC2_DATA:    u16 = 0xA1;

const PIC_ICW1: u8 = 0x11;     // ICW4 needed, cascade mode
const PIC_ICW4_8086: u8 = 0x01;

/// Remap the PIC to move IRQ vectors away from CPU exception vectors (0–31).
///
/// The master PIC's primary IRQs (0–7) get remapped to `master_offset`–`master_offset+7`.
/// The slave PIC's IRQs (8–15) get remapped to `slave_offset`–`slave_offset+7`.
///
/// Standard usage: remap master to 0x20 (32) and slave to 0x28 (40).
pub fn remap_pic(master_offset: u8, slave_offset: u8) {
    // Save current masks
    let mask1 = unsafe { inb(PIC1_DATA) };
    let mask2 = unsafe { inb(PIC2_DATA) };

    // Begin initialization (ICW1)
    unsafe {
        outb(PIC1_COMMAND, PIC_ICW1);
        outb(PIC2_COMMAND, PIC_ICW1);

        // ICW2: vector offsets
        outb(PIC1_DATA, master_offset);
        outb(PIC2_DATA, slave_offset);

        // ICW3: cascade configuration
        outb(PIC1_DATA, 0x04);   // slave on IRQ2 (bit mask 0x04)
        outb(PIC2_DATA, 0x02);   // cascade identity = 2

        // ICW4: 8086 mode
        outb(PIC1_DATA, PIC_ICW4_8086);
        outb(PIC2_DATA, PIC_ICW4_8086);

        // Restore saved masks (all interrupts still masked after init)
        outb(PIC1_DATA, mask1);
        outb(PIC2_DATA, mask2);
    }
}

// Port I/O wrappers (mirrored from serial.rs for independence)
unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!("outb %al, %dx", in("al") val, in("dx") port, options(att_syntax));
}

unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    core::arch::asm!("inb %dx, %al", out("al") val, in("dx") port, options(att_syntax));
    val
}

//------------------------------------------------------------------------------
// Public API
//------------------------------------------------------------------------------

/// Global IDT instance.  Initialised once by `init_interrupts()`.
static mut IDT: Idt = Idt::new();

/// Load the IDT via the `lidt` instruction.
unsafe fn load_idt(idt: &Idt) {
    let desc = IdtDescriptor {
        limit: (core::mem::size_of::<Idt>() - 1) as u16,
        base: idt as *const Idt as u64,
    };
    // AT&T syntax: lidt loads from memory operand
    core::arch::asm!("lidt ({0})", in(reg) &desc, options(nostack, preserves_flags));
}

/// Enable interrupts (sti).
pub unsafe fn enable_interrupts() {
    core::arch::asm!("sti", options(nostack, preserves_flags));
}

/// Disable interrupts (cli).
pub unsafe fn disable_interrupts() {
    core::arch::asm!("cli", options(nostack, preserves_flags));
}

/// Initialise the interrupt system:
/// 1. Set up all 32 CPU exception handlers in the IDT
/// 2. Remap the PIC to avoid vector collision (master → 0x20, slave → 0x28)
/// 3. Load the IDT (`lidt`)
///
/// Note: Interrupts are NOT enabled by default.  Call `enable_interrupts()`
/// when ready.
pub fn init_interrupts() {
    let selector = 0x08;  // kernel code segment from GDT

    unsafe {
        let idt = &mut IDT;

        // Set up handlers for CPU exceptions 0–31
        for i in 0..32 {
            let handler = ISR_STUBS[i] as u64;
            idt.set(i, handler, selector, 0);  // dpl = 0 (kernel)
        }

        // Remap PIC to put IRQs at vectors 0x20–0x2F (32–47)
        remap_pic(0x20, 0x28);

        // Load the IDT
        load_idt(idt);
    }
}

//------------------------------------------------------------------------------
// Panic-like exception message
//------------------------------------------------------------------------------

/// Exception name table (index by int_no 0–31).
const EXCEPTION_NAMES: &[&str] = &[
    "Divide-by-zero",              // 0
    "Debug",                       // 1
    "Non-maskable Interrupt",      // 2
    "Breakpoint",                  // 3
    "Overflow",                    // 4
    "Bound Range Exceeded",        // 5
    "Invalid Opcode",              // 6
    "Device Not Available",        // 7
    "Double Fault",                // 8
    "Coprocessor Segment Overrun", // 9
    "Invalid TSS",                 // 10
    "Segment Not Present",         // 11
    "Stack-Segment Fault",         // 12
    "General Protection Fault",    // 13
    "Page Fault",                  // 14
    "Reserved",                    // 15
    "x87 FPU Error",               // 16
    "Alignment Check",             // 17
    "Machine Check",               // 18
    "SIMD FP Exception",           // 19
    "Virtualization Exception",    // 20
    "Control Protection",          // 21
    "Reserved",                    // 22
    "Reserved",                    // 23
    "Reserved",                    // 24
    "Reserved",                    // 25
    "Reserved",                    // 26
    "Reserved",                    // 27
    "Reserved",                    // 28
    "Reserved",                    // 29
    "Security Exception",          // 30
    "Reserved",                    // 31
];

/// Format a hex value into a fixed-size buffer (no_std compatible).
fn hex_str(val: u64) -> [u8; 18] {
    let mut buf = [0u8; 18];
    buf[0] = b'0';
    buf[1] = b'x';
    let hex_chars = b"0123456789ABCDEF";
    for i in 0..16 {
        let nibble = ((val >> (60 - i * 4)) & 0xF) as usize;
        buf[i + 2] = hex_chars[nibble];
    }
    buf
}

//------------------------------------------------------------------------------
// The handler called from interrupts.asm isr_common
//------------------------------------------------------------------------------

/// Called from the assembly `isr_common` handler with a pointer to the
/// full interrupt frame on the stack.
#[no_mangle]
pub extern "C" fn interrupt_handler(frame: &InterruptFrame) {
    let mut serial = SerialPort::new();
    serial.init();

    let int_no = frame.int_no;
    let name = if (int_no as usize) < EXCEPTION_NAMES.len() {
        EXCEPTION_NAMES[int_no as usize]
    } else {
        "Unknown"
    };

    // Build panic message manually (avoid fmt machinery in exception context)
    serial.writestrs(&["\n========================================\n"]);
    serial.writestrs(&["VIBIX: EXCEPTION: "]);
    serial.writestrs(&[name, " (", "\n"]);
    // Print int_no - use integer to byte conversion
    let _ = write!(serial, "VIBIX: Exception #{}: {}\n", int_no, name);

    let rip_hex = core::str::from_utf8(&hex_str(frame.rip)).unwrap_or("???");
    let cs_hex  = core::str::from_utf8(&hex_str(frame.cs)).unwrap_or("???");
    let rflags_hex = core::str::from_utf8(&hex_str(frame.rflags)).unwrap_or("???");
    let err_hex = core::str::from_utf8(&hex_str(frame.err_code)).unwrap_or("???");

    serial.writestrs(&["VIBIX:   RIP: ", rip_hex, "\n"]);
    serial.writestrs(&["VIBIX:    CS: ", cs_hex, "\n"]);
    serial.writestrs(&["VIBIX: RFLAGS: ", rflags_hex, "\n"]);
    serial.writestrs(&["VIBIX:   ERR: ", err_hex, "\n"]);

    // Print saved registers
    serial.writestrs(&["VIBIX: Registers:\n"]);
    let rax_hex = core::str::from_utf8(&hex_str(frame.regs.rax)).unwrap_or("???");
    let rbx_hex = core::str::from_utf8(&hex_str(frame.regs.rbx)).unwrap_or("???");
    let rcx_hex = core::str::from_utf8(&hex_str(frame.regs.rcx)).unwrap_or("???");
    let rdx_hex = core::str::from_utf8(&hex_str(frame.regs.rdx)).unwrap_or("???");
    let rsi_hex = core::str::from_utf8(&hex_str(frame.regs.rsi)).unwrap_or("???");
    let rdi_hex = core::str::from_utf8(&hex_str(frame.regs.rdi)).unwrap_or("???");
    let rbp_hex = core::str::from_utf8(&hex_str(frame.regs.rbp)).unwrap_or("???");
    let r8_hex  = core::str::from_utf8(&hex_str(frame.regs.r8)).unwrap_or("???");
    let r9_hex  = core::str::from_utf8(&hex_str(frame.regs.r9)).unwrap_or("???");
    let r10_hex = core::str::from_utf8(&hex_str(frame.regs.r10)).unwrap_or("???");
    let r11_hex = core::str::from_utf8(&hex_str(frame.regs.r11)).unwrap_or("???");
    let r12_hex = core::str::from_utf8(&hex_str(frame.regs.r12)).unwrap_or("???");
    let r13_hex = core::str::from_utf8(&hex_str(frame.regs.r13)).unwrap_or("???");
    let r14_hex = core::str::from_utf8(&hex_str(frame.regs.r14)).unwrap_or("???");
    let r15_hex = core::str::from_utf8(&hex_str(frame.regs.r15)).unwrap_or("???");

    serial.writestrs(&["VIBIX:   RAX: ", rax_hex, "\n"]);
    serial.writestrs(&["VIBIX:   RBX: ", rbx_hex, "\n"]);
    serial.writestrs(&["VIBIX:   RCX: ", rcx_hex, "\n"]);
    serial.writestrs(&["VIBIX:   RDX: ", rdx_hex, "\n"]);
    serial.writestrs(&["VIBIX:   RSI: ", rsi_hex, "\n"]);
    serial.writestrs(&["VIBIX:   RDI: ", rdi_hex, "\n"]);
    serial.writestrs(&["VIBIX:   RBP: ", rbp_hex, "\n"]);
    serial.writestrs(&["VIBIX:    R8: ", r8_hex, "\n"]);
    serial.writestrs(&["VIBIX:    R9: ", r9_hex, "\n"]);
    serial.writestrs(&["VIBIX:   R10: ", r10_hex, "\n"]);
    serial.writestrs(&["VIBIX:   R11: ", r11_hex, "\n"]);
    serial.writestrs(&["VIBIX:   R12: ", r12_hex, "\n"]);
    serial.writestrs(&["VIBIX:   R13: ", r13_hex, "\n"]);
    serial.writestrs(&["VIBIX:   R14: ", r14_hex, "\n"]);
    serial.writestrs(&["VIBIX:   R15: ", r15_hex, "\n"]);

    serial.writestrs(&["========================================\n"]);

    // Halt the system for all exception handlers (debug, breakpoint, etc.
    // could be handled differently in the future).
    loop {
        unsafe { core::arch::asm!("hlt", options(nomem, nostack)) }
    }
}
