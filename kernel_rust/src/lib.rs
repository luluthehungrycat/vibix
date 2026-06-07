//==============================================================================
// lib.rs — VIBIX 64-bit kernel entry point (Rust)
//
// Called from kernel64_entry.asm after BSS zeroing and stack setup.
// Provides kernel_main, panic handler, and top-level boot sequence.
//==============================================================================

#![no_std]

mod serial;
mod pmm;

use core::panic::PanicInfo;

//------------------------------------------------------------------------------
// Kernel entry — called from assembly (kernel64_entry.asm)
//------------------------------------------------------------------------------
#[no_mangle]
pub extern "C" fn kernel_main() -> ! {
    // Initialise serial port (COM1, 115200 8N1)
    let mut serial = serial::SerialPort::new();
    serial.init();

    // Boot banner
    serial.writestrs(&[
        "\n",
        "========================================\n",
        "  VIBIX — UNIXoid Kernel (Rust Port)\n",
        "========================================\n",
        "\n",
        "VIBIX: Kernel alive!\n",
    ]);

    // Physical Memory Manager
    // For now: manage the range 1 MB → 256 MB.
    // Future: parse the Multiboot memory map for the real layout.
    let mut pmm = pmm::PmmAllocator::new();
    pmm.init(0x100000, 0x10000000);
    pmm.test(&mut serial);

    serial.writestrs(&[
        "VIBIX: Boot sequence complete — entering idle loop.\n",
    ]);

    // Idle
    loop {
        unsafe { core::arch::asm!("hlt", options(nomem, nostack)) }
    }
}

//------------------------------------------------------------------------------
// Panic handler
//------------------------------------------------------------------------------
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // Write directly to serial without using fmt machinery (may be broken).
    let mut serial = serial::SerialPort::new();
    serial.writestrs(&["VIBIX: PANIC: "]);
    if let Some(msg) = info.message().as_str() {
        serial.writestrs(&[msg, "\n"]);
    } else {
        serial.writestrs(&["<non-string panic message>\n"]);
    }
    loop {
        unsafe { core::arch::asm!("hlt", options(nomem, nostack)) }
    }
}
