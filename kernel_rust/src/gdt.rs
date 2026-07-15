//==============================================================================
// gdt.rs — Global Descriptor Table and Task State Segment
//
// Provides:
//   - Full 64-bit GDT with Ring 0/3 segments
//   - TSS with RSP0 for syscall stack
//   - MSR setup for SYSCALL/SYSRET (STAR, LSTAR, SF_MASK)
//   - gdt::init() to load the GDT, TSS, and program MSRs
//==============================================================================

use core::arch::asm;

//------------------------------------------------------------------------------
// GDT entry builder
//------------------------------------------------------------------------------

/// Build a code or data segment descriptor (S=1).
const fn seg_entry(access: u8, flags: u8) -> u64 {
    (flags as u64) << 48 | (access as u64) << 40
}

//------------------------------------------------------------------------------
// GDT constants
//------------------------------------------------------------------------------

/// Null descriptor.
const GDT_NULL: u64 = 0;

/// Kernel code 64-bit: P=1, DPL=0, S=1, E=1, R=1, L=1 — Access=0x9A, Flags=0x20.
const GDT_KERNEL_CODE: u64 = seg_entry(0x9A, 0x20);

/// Kernel data: P=1, DPL=0, S=1, E=0, W=1 — Access=0x92.
const GDT_KERNEL_DATA: u64 = seg_entry(0x92, 0x00);

/// User data: P=1, DPL=3, S=1, E=0, W=1 — Access=0xF2.
const GDT_USER_DATA: u64 = seg_entry(0xF2, 0x00);

/// User code 64-bit: P=1, DPL=3, S=1, E=1, R=1, L=1 — Access=0xFA, Flags=0x20.
const GDT_USER_CODE: u64 = seg_entry(0xFA, 0x20);

/// SYSRET code 64-bit: same as USER_CODE but at a different index for SYSRET selector math.
/// P=1, DPL=3, S=1, E=1, R=1, L=1 — Access=0xFA, Flags=0x20.
const GDT_SYSRET_CODE: u64 = seg_entry(0xFA, 0x20);

/// SYSRET data: same as USER_DATA but at a different index.
/// P=1, DPL=3, S=1, E=0, W=1 — Access=0xF2.
const GDT_SYSRET_DATA: u64 = seg_entry(0xF2, 0x00);

/// Selector values (index << 3).
pub const KERNEL_CS: u16 = 1 << 3;     // 0x08
pub const KERNEL_DS: u16 = 2 << 3;     // 0x10
#[allow(dead_code)]
pub const USER_DS:  u16 = 3 << 3;      // 0x18
#[allow(dead_code)]
pub const USER_CS:  u16 = 4 << 3;      // 0x20

/// User-mode selectors with RPL=3 ORed in.
/// Use these in iretq frames and segment loads targeting Ring 3.
#[allow(dead_code)]
pub const USER_DS_RPL3: u16 = USER_DS | 3;  // 0x1B
#[allow(dead_code)]
pub const USER_CS_RPL3: u16 = USER_CS | 3;  // 0x23

/// SYSRET-code selector (index 5).
pub const SYSRET_CS: u16 = 5 << 3;     // 0x28

/// SYSRET-data selector (index 6).
pub const SYSRET_DS: u16 = 6 << 3;     // 0x30

//------------------------------------------------------------------------------
// TSS (Task State Segment) — 104 bytes
//------------------------------------------------------------------------------

/// x86_64 Task State Segment.
///
/// Minimum size is 104 bytes (0x68).  We only need RSP0 set for SYSCALL entry.
#[repr(C, packed)]
struct TaskStateSegment {
    reserved0: u32,        // 0x00
    rsp0:     u64,         // 0x04 — Kernel stack for Ring 0 transitions
    rsp1:     u64,         // 0x0C
    rsp2:     u64,         // 0x14
    reserved1: u64,        // 0x1C
    ist1:     u64,         // 0x24 — Interrupt Stack Table 1
    ist2:     u64,         // 0x2C
    ist3:     u64,         // 0x34
    ist4:     u64,         // 0x3C
    ist5:     u64,         // 0x44
    ist6:     u64,         // 0x4C
    ist7:     u64,         // 0x54
    reserved2: u64,        // 0x5C
    iomap_base: u16,       // 0x64
}

impl TaskStateSegment {
    const fn new() -> Self {
        Self {
            reserved0: 0,
            rsp0: 0,
            rsp1: 0,
            rsp2: 0,
            reserved1: 0,
            ist1: 0, ist2: 0, ist3: 0, ist4: 0,
            ist5: 0, ist6: 0, ist7: 0,
            reserved2: 0,
            iomap_base: 0,
        }
    }
}

/// Build a TSS descriptor (two consecutive 8-byte GDT entries).
///
/// Returns (low, high) — the two descriptor qwords.
fn tss_descriptor(tss_addr: u64, size: u32) -> (u64, u64) {
    let limit = size.wrapping_sub(1);   // limit = size - 1

    let low = (limit as u64 & 0xFFFF)                              // limit[15:0]
        | ((tss_addr & 0xFFFFFF) << 16)                            // base[23:0]
        | ((tss_addr & 0xFF000000) << 32)                          // base[31:24]
        | (0x89u64 << 40)                                          // P=1, DPL=0, S=0, Type=0x9
        | (((limit >> 16) as u64 & 0xF) << 48);                   // limit[19:16]

    let high = (tss_addr >> 32) as u64;                            // base[63:32]

    (low, high)
}

//------------------------------------------------------------------------------
// Full GDT table
//------------------------------------------------------------------------------

/// Full GDT: null, kernel code, kernel data, user data, user code,
/// sysret code, sysret data, TSS low, TSS high.
#[repr(C, align(16))]
pub struct Gdt {
    entries: [u64; 9],
}

impl Gdt {
    const fn new() -> Self {
        Self {
            entries: [
                GDT_NULL,          // [0]
                GDT_KERNEL_CODE,   // [1]
                GDT_KERNEL_DATA,   // [2]
                GDT_USER_DATA,     // [3]
                GDT_USER_CODE,     // [4]
                GDT_SYSRET_CODE,   // [5] — SYSRET CS
                GDT_SYSRET_DATA,   // [6] — SYSRET SS
                0,                 // [7] — TSS low (set at runtime)
                0,                 // [8] — TSS high (set at runtime)
            ],
        }
    }

    fn install_tss(&mut self, tss: &TaskStateSegment) {
        let addr = tss as *const TaskStateSegment as u64;
        let size = core::mem::size_of::<TaskStateSegment>() as u32;
        let (low, high) = tss_descriptor(addr, size);
        self.entries[7] = low;   // TSS descriptor (index 7)
        self.entries[8] = high;
    }
}

/// GDT pointer structure for `lgdt`.
#[repr(C, packed)]
struct GdtPointer {
    limit: u16,
    base: u64,
}

//------------------------------------------------------------------------------
// Public API
//------------------------------------------------------------------------------

/// Global GDT instance.
static mut GDT: Gdt = Gdt::new();

/// Global TSS instance — lives here because it needs a stable address.
static mut TSS: TaskStateSegment = TaskStateSegment::new();

/// Update TSS.RSP0 — called by the scheduler on each context switch
/// so that interrupts from Ring 3 land on the current process's kernel stack.
///
/// # Safety
///
/// Must be called with interrupts disabled.
pub unsafe fn set_rsp0(rsp0: u64) {
    TSS.rsp0 = rsp0;
}

/// Load GDT via `lgdt` and reload segment registers.
///
/// # Safety
///
/// Must be called once, with interrupts disabled, before any Ring 3 transition.
unsafe fn load_gdt(gdt: &Gdt) {
    let ptr = GdtPointer {
        limit: (core::mem::size_of::<Gdt>() - 1) as u16,
        base: gdt as *const Gdt as u64,
    };

    asm!(
        // Load the new GDT
        "lgdt [{ptr}]",

        // Reload CS via far return
        "push {kernel_cs}",
        "lea rax, [2f + rip]",
        "push rax",
        "retfq",
        "2:",

        // Reload data segments
        "mov ds, {kernel_ds}",
        "mov es, {kernel_ds}",
        "mov ss, {kernel_ds}",
        // fs and gs can remain as-is (0)

        ptr = in(reg) &ptr,
        kernel_cs = in(reg) KERNEL_CS as u64,
        kernel_ds = in(reg) KERNEL_DS as u64,
        out("rax") _,
        options(nostack, preserves_flags),
    );
}

/// Load the TSS via `ltr`.
///
/// # Safety
///
/// Requires a valid TSS descriptor at GDT index 7.
unsafe fn load_tss() {
    // TSS descriptor is at GDT index 7 → selector = 7 << 3 = 0x38
    asm!("ltr {0:x}", in(reg) 0x38u16, options(nostack, preserves_flags));
}

/// Initialise MSRs for SYSCALL/SYSRET.
///
/// SYSRETQ in 64-bit mode (Intel SDM Vol 2B, SYSRET):
///   CS = STAR[63:48] & 0xFFFC, then RPL=3 forced
///   SS = (STAR[63:48] + 8) & 0xFFFC, then RPL=3 forced
///
/// GDT layout:
///   [0] NULL
///   [1] KERNEL_CODE  (0x08)
///   [2] KERNEL_DATA  (0x10)
///   [3] USER_DATA    (0x18)
///   [4] USER_CODE    (0x20)
///   [5] SYSRET_CODE  (0x28) — DPL=3, L=1
///   [6] SYSRET_DATA  (0x30) — DPL=3, W=1
///   [7] TSS low      (0x38)
///   [8] TSS high     (0x40)
///
/// STAR[63:48] = 0x13 (compatibility):
///   SYSRET CS = 0x13 (GDT index 2, data descriptor — non-standard but works)
///   SYSRET SS = 0x1B (GDT index 3, USER_DATA | 3)
///
/// STAR[47:32] = KERNEL_CS = 0x08:
///   SYSCALL CS = 0x08 (kernel code)
///   SYSCALL SS = 0x10 (kernel data)
///
/// LSTAR = address of syscall_entry.
/// SF_MASK = mask RFLAGS IF to prevent interrupts during syscall.
unsafe fn setup_syscall_msrs(syscall_entry: u64) {
    // SYSRETQ computes:
    //   CS = STAR[63:48] & 0xFFFC, then RPL forced to 3
    //   SS = (STAR[63:48] + 8) & 0xFFFC, then RPL forced to 3
    //
    // With STAR[63:48] = 0x13 (compatibility value):
    //   SYSRET CS = 0x13 (GDT index 2, data descriptor — works but non-standard)
    //   SYSRET SS = 0x1B (GDT index 3, USER_DATA with RPL=3)
    const STAR_SYSRET_SS_BASE: u64 = 0x13;
    let star: u64 = (STAR_SYSRET_SS_BASE) << 48   // SYSRET SS base
                  | (KERNEL_CS as u64) << 32;      // SYSCALL CS

    asm!(
        "wrmsr",
        in("ecx") 0xC0000081u32,   // IA32_STAR
        in("eax") star as u32,
        in("edx") (star >> 32) as u32,
        options(nostack, preserves_flags),
    );

    asm!(
        "wrmsr",
        in("ecx") 0xC0000082u32,   // IA32_LSTAR
        in("eax") syscall_entry as u32,
        in("edx") (syscall_entry >> 32) as u32,
        options(nostack, preserves_flags),
    );

    // SF_MASK: mask IF (bit 9) during syscall — interrupts disabled in ring 0
    let fmask: u64 = 0x200u64;     // IF = bit 9
    asm!(
        "wrmsr",
        in("ecx") 0xC0000084u32,   // IA32_FMASK
        in("eax") fmask as u32,
        in("edx") (fmask >> 32) as u32,
        options(nostack, preserves_flags),
    );
}

//------------------------------------------------------------------------------
// Initialization
//------------------------------------------------------------------------------

// The top of the 16 KiB kernel stack set up by kernel64_entry.asm.
// Used for TSS.RSP0 so that interrupts from Ring 3 land on a valid stack.
extern "C" {
    static stack_top: u8;
}

/// Initialise the GDT, TSS, and syscall MSRs.
///
/// Must be called once, early in boot, after the stack is set up but before
/// any Ring 3 transition.  Interrupts must be disabled.
///
/// `syscall_entry` is the 64-bit virtual address of the syscall entry point.
/// Pass `0` to skip MSR setup (if bootstrapping without syscall).
pub fn init(syscall_entry: u64) {
    unsafe {
        // Set TSS.RSP0 to the actual top of the 16 KiB kernel stack so that
        // interrupts from Ring 3 have plenty of room.
        TSS.rsp0 = &stack_top as *const u8 as u64;

        // Install TSS descriptor into GDT
        let gdt: *mut Gdt = &raw mut GDT;
        let tss: *mut TaskStateSegment = &raw mut TSS;
        (*gdt).install_tss(&*tss);

        // Load the GDT
        load_gdt(&*gdt);

        // Load TSS
        load_tss();

        // Set up syscall MSRs if an entry point was provided
        if syscall_entry != 0 {
            setup_syscall_msrs(syscall_entry);

            // Enable SYSCALL (EFER.SCE bit 0)
            let efer_low: u32;
            let efer_high: u32;
            asm!(
                "rdmsr",
                in("ecx") 0xC0000080u32,   // IA32_EFER
                out("eax") efer_low,
                out("edx") efer_high,
                options(nostack),
            );
            let efer_val = (efer_high as u64) << 32 | efer_low as u64;
            let efer_sce = efer_val | 1;    // set bit 0 (SCE)
            asm!(
                "wrmsr",
                in("ecx") 0xC0000080u32,
                in("eax") efer_sce as u32,
                in("edx") (efer_sce >> 32) as u32,
                options(nostack, preserves_flags),
            );
        }
    }
}
