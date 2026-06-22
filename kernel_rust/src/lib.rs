//==============================================================================
// lib.rs — VIBIX 64-bit kernel entry point (Rust)
//
// Called from kernel64_entry.asm after BSS zeroing and stack setup.
// Provides kernel_main, panic handler, and top-level boot sequence.
//==============================================================================

#![no_std]

mod serial;
mod pmm;
mod kmm;
mod interrupts;
mod multiboot;
mod pit;
mod keyboard;
mod fb;
mod gdt;
mod syscall;
mod paging;

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
    let mut pmm = pmm::PmmAllocator::new();

    // Try to initialise from Multiboot memory map (QEMU provides one).
    let mbi = multiboot::get_mbi_ptr();
    if mbi != 0 && multiboot::apply_mmap_to_pmm(&mut pmm, mbi) {
        multiboot::print_mmap(mbi, &mut serial);
    } else {
        // Fallback: no Multiboot mmap — use hardcoded range.
        serial.writestrs(&["VIBIX: No Multiboot mmap — using hardcoded range.\n"]);
        pmm.init(0x100000, 0x10000000);
    }

    // Reserve the kernel's own memory (0x200000 + 512 KiB for code + BSS + page tables + stack).
    pmm.reserve(0x200000, 0x80000);

    pmm.test(&mut serial);

    // Kernel Heap Allocator
    kmm::init(&mut pmm);
    kmm::test(&mut serial);

    // Page Table Manager
    paging::test(&mut pmm, &mut serial);

    // Framebuffer (Bochs VBE direct programming)
    match fb::init(&mut pmm, &mut serial) {
        Some(fb) => {
            // Draw boot graphics
            fb.clear(0x00101A);  // dark navy background
            fb.draw_string(24, 20, "VIBIX", 0x00FFAA, Some(0x00101A));
            fb.draw_string(24, 40, "UNIXoid Kernel (Rust Port)", 0x888888, Some(0x00101A));
            fb.draw_string(24, 60, "VBE Framebuffer", 0xAAAAAA, Some(0x00101A));

            // Draw a test pattern — coloured rectangles
            let colours = [0xFF0000u32, 0x00FF00, 0x0000FF, 0xFFFF00, 0xFF00FF, 0x00FFFF];
            let bar_w = fb.width / 6;
            for i in 0..6 {
                fb.fill_rect(i * bar_w, fb.height - 32, (i + 1) * bar_w - 1, fb.height - 1, colours[i as usize]);
            }

            serial.writestrs(&["VIBIX: Framebuffer initialised.\n"]);
        }
        None => {
            serial.writestrs(&["VIBIX: No framebuffer — continuing with serial only.\n"]);
        }
    }

    serial.writestrs(&[
        "VIBIX: Initialising interrupts...\n",
    ]);

    interrupts::init_interrupts();
    serial.writestrs(&["VIBIX: IDT loaded, PIC remapped.\n"]);

    // Program PIT timer (IRQ0 at ~100 Hz)
    pit::init();
    serial.writestrs(&["VIBIX: PIT timer initialised at 100 Hz.\n"]);

    // GDT, TSS, and syscall MSR setup
    serial.writestrs(&["VIBIX: Loading GDT/TSS and enabling SYSCALL.\n"]);
    extern "C" {
        fn syscall_entry();
    }
    gdt::init(syscall_entry as *const () as u64);
    syscall::init();

    // Enable interrupts — timer ticks will begin immediately
    serial.writestrs(&["VIBIX: Enabling interrupts.\n"]);
    unsafe { interrupts::enable_interrupts(); }

    serial.writestrs(&[
        "VIBIX: Boot sequence complete — entering idle loop.\n",
    ]);

    // Idle — hlt wakes on each IRQ0 tick, then immediately halts again
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
