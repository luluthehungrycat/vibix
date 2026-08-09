//==============================================================================
// lib.rs — VIBIX 64-bit kernel entry point (Rust)
//
// Called from kernel64_entry.asm after BSS zeroing and stack setup.
// Provides kernel_main, panic handler, and top-level boot sequence.
//==============================================================================

#![no_std]

mod elf;
mod fb;
mod gdt;
mod interrupts;
mod keyboard;
mod kmm;
mod multiboot;
mod paging;
mod pit;
mod pmm;
mod process;
mod serial;
mod signal;
mod syscall;

mod vfs;
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

    // Initialise global PMM so runtime code (brk, ELF loader, etc.) can allocate.
    pmm::init_global(&pmm);

    // Kernel Heap Allocator
    kmm::init(&mut pmm);
    kmm::test(&mut serial);

    // Page Table Manager
    paging::test(&mut pmm, &mut serial);
    if cfg!(feature = "debug") {
        elf::test_provenance_arena(&mut serial);
    }

    // Framebuffer (Bochs VBE direct programming)
    match fb::init(&mut pmm, &mut serial) {
        Some(fb) => {
            // Draw boot graphics
            fb.clear(0x00101A); // dark navy background
            fb.draw_string(24, 20, "VIBIX", 0x00FFAA, Some(0x00101A));
            fb.draw_string(
                24,
                40,
                "UNIXoid Kernel (Rust Port)",
                0x888888,
                Some(0x00101A),
            );
            fb.draw_string(24, 60, "VBE Framebuffer", 0xAAAAAA, Some(0x00101A));

            // Draw a test pattern — coloured rectangles
            let colours = [
                0xFF0000u32,
                0x00FF00,
                0x0000FF,
                0xFFFF00,
                0xFF00FF,
                0x00FFFF,
            ];
            let bar_w = fb.width / 6;
            for i in 0..6 {
                fb.fill_rect(
                    i * bar_w,
                    fb.height - 32,
                    (i + 1) * bar_w - 1,
                    fb.height - 1,
                    colours[i as usize],
                );
            }

            serial.writestrs(&["VIBIX: Framebuffer initialised.\n"]);
        }
        None => {
            serial.writestrs(&["VIBIX: No framebuffer — continuing with serial only.\n"]);
        }
    }

    serial.writestrs(&["VIBIX: Initialising interrupts...\n"]);

    interrupts::init_interrupts();
    serial.writestrs(&["VIBIX: IDT loaded, PIC remapped.\n"]);

    // Program PIT timer (IRQ0 at ~100 Hz)
    pit::init();
    serial.writestrs(&["VIBIX: PIT timer initialised at 100 Hz.\n"]);

    // Initialize PS/2 keyboard
    keyboard::init();
    serial.writestrs(&["VIBIX: PS/2 keyboard ready.\n"]);
    interrupts::unmask_irq(1);
    serial.writestrs(&["VIBIX: Keyboard IRQ unmasked -- PS/2 input active.\n"]);

    // Unmask timer — PIT will fire IRQ0 after STI
    interrupts::unmask_irq(0);
    serial.writestrs(&["VIBIX: Timer IRQ unmasked.\n"]);

    // GDT, TSS, and syscall MSR setup
    serial.writestrs(&["VIBIX: Loading GDT/TSS and enabling SYSCALL.\n"]);
    extern "C" {
        fn syscall_entry();
    }
    gdt::init(syscall_entry as *const () as u64);
    syscall::init();
    // Virtual File System
    serial.writestrs(&["VIBIX: Initialising VFS...\n"]);
    unsafe {
        vfs::vfs_init();
    }
    serial.writestrs(&["VIBIX: VFS ready.\n"]);
    // Enable interrupts — timer ticks will begin immediately
    serial.writestrs(&["VIBIX: Enabling interrupts.\n"]);
    unsafe {
        interrupts::enable_interrupts();
    }

    serial.writestrs(&["VIBIX: Boot sequence complete — spawning PID 1.\n"]);

    // Create the init process
    let init_pid = process::spawn_init(&mut pmm);
    serial.writestrs(&["VIBIX: Created PID 1 (init).\n"]);

    // Start the scheduler — never returns
    serial.writestrs(&["VIBIX: Starting scheduler...\n"]);
    unsafe {
        process::start_scheduler(init_pid);
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

//------------------------------------------------------------------------------
// Debug: print iretq frame values right before iretq instruction.
// Called from assembly (irq_common, .exit_or_block) BEFORE iretq.
//
// rsp — kernel RSP pointing at RIP in the iretq frame
// path — 1 = irq_common, 2 = .exit_or_block
//------------------------------------------------------------------------------
#[no_mangle]
pub extern "C" fn debug_iretq_ss(rsp: u64, path: u64) {
    if cfg!(feature = "debug") {
        // Use the already-initialized serial port (no init() call needed).
        // init() clears the TX FIFO which fragments output and causes
        // invalid UTF-8 in the test harness.
        let mut serial = serial::SerialPort::new();

        unsafe {
            // Frame layout at iretq: RSP points at RIP
            let rip = core::ptr::read_volatile((rsp) as *const u64);
            let cs = core::ptr::read_volatile((rsp + 8) as *const u64);
            let rflags = core::ptr::read_volatile((rsp + 16) as *const u64);
            let user_rsp = core::ptr::read_volatile((rsp + 24) as *const u64);
            let ss = core::ptr::read_volatile((rsp + 32) as *const u64);

            // GPR slots: RSP-136=RAX, RSP-128=RCX, RSP-16=int_no, RSP-8=err_code
            let rax = core::ptr::read_volatile((rsp.wrapping_sub(136)) as *const u64);
            let rcx = core::ptr::read_volatile((rsp.wrapping_sub(128)) as *const u64);
            let int_no = core::ptr::read_volatile((rsp.wrapping_sub(16)) as *const u64);
            let err_code = core::ptr::read_volatile((rsp.wrapping_sub(8)) as *const u64);

            // Use writestrs to avoid format_args! / write! macro overhead
            // which might reference pageable memory at interrupt time.
            hex_out(&mut serial, b"DBG iretq_pre: path=", path);
            hex_out(&mut serial, b" RSP=", rsp);
            serial.writestrs(&["\n"]);

            hex_out(&mut serial, b"  RIP=", rip);
            hex_out(&mut serial, b" CS=", cs);
            hex_out(&mut serial, b" RFLAGS=", rflags);
            hex_out(&mut serial, b" userRSP=", user_rsp);
            serial.writestrs(&["\n"]);

            hex_out(&mut serial, b"  SS=", ss);
            hex_out(&mut serial, b" int=", int_no);
            hex_out(&mut serial, b" err=", err_code);
            hex_out(&mut serial, b" RAX=", rax);
            hex_out(&mut serial, b" RCX=", rcx);
            serial.writestrs(&["\n"]);
        }
    }
}

/// Write a label + u64 hex value to serial without fmt machinery.
fn hex_out(serial: &mut serial::SerialPort, label: &[u8], val: u64) {
    serial.writestrs(&[core::str::from_utf8(label).unwrap_or("??")]);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut buf = [0u8; 18];
    buf[0] = b'0';
    buf[1] = b'x';
    for i in 0..16 {
        buf[17 - i] = HEX[((val >> (4 * i)) & 0xF) as usize];
    }
    serial.writestrs(&[core::str::from_utf8(&buf).unwrap_or("??")]);
}
