# Next Session: Per-Process Page Tables + vish Rust Frontend

## Repository State

Branch: `scheduler-phase1` → PR [#6](https://github.com/luluthehungrycat/vibix/pull/6) to `main`
All commits pushed. Test: `make test` → 15/15 passes. `make test_vibit` → 7/7 passes.

## Current Architecture

- **Single address space**: All processes share one PML4 page table
- **Kernel modifies active page tables directly** via `paging::map_4k()`
- **No TLB flush needed on context switch** (no CR3 switch)
- **User code at** `0x2000000`, **user stack at** `0x2002000` (one page)
- **Kernel stack**: 12KB per process (3 pages via `alloc_pages(3)`)

## Task 1: Per-Process Page Tables

### Goal
Give each process its own PML4 table so they can't access each other's memory.

### Step-by-Step Plan

**Step 1: Add `pml4` field to Process struct** (`kernel_rust/src/process.rs`)
- Add `pml4: *mut PageTable` (or `Option<&'static mut PageTable>`) field
- Allocate a new PML4 page for each process at creation time
- Copy kernel mappings from the current active PML4
- Initialize PID 1 and PID 2 (idle) with their own PML4

**Step 2: Switch CR3 on context switch**
- In `scheduler_tick()` (line 450): after picking next process, call `write_cr3(next.pml4)`
- In `scheduler_switch_exit()` (line 479): same thing
- In `set_syscall_kstack()` (line 130-132): also switch CR3? Or ensure TSS.RSP0 is per-process
- The `syscall_entry.asm` entry saves user RSP and switches to kernel stack — but kernel code runs in the CURRENT page tables. Need to ensure kernel mappings are identical in ALL page tables.

**Step 3: Update load_flat_binary** (`load_flat_binary` at line 233)
- Currently uses `paging::map_4k()` which modifies the ACTIVE page tables
- Needs to modify the PROCESS's page tables instead
- Two approaches:
  - **A**: Switch CR3 to the target process's PML4 before mapping (expensive)
  - **B**: Map all process PML4s into a kernel-reserved virtual address range
    (e.g., at `0xFFFF8000_00000000`), then modify through that mapping

**Step 4: Update ELF loader** (`elf.rs` line 88)
- Same issue as load_flat_binary — modifies active page tables
- Needs per-process PML4 access

**Step 5: Update fork** (`sys_fork` at line 527)
- Child needs its own PML4
- Copy kernel mappings from parent
- Create fresh user mappings (COW or copy)

**Step 6: Update tty_wake** (`tty.rs` push_byte at line 169)
- When waking a blocked process, the kernel reads/writes `process_mut(pid)`
- `process_mut` doesn't access user space — it accesses the PROCESS_TABLE
- So no page table change needed here

### Key Challenge: Accessing Target Process Page Tables
The kernel runs in the ACTIVE page tables. To modify another process's page tables,
you need to either:
- **Temporarily switch CR3** to that process's PML4 (like Linux does for process
  address space access). This is simple but expensive.
- **Reserve a kernel virtual address range** where all process PML4s are mapped.
  VIBIX kernel is mapped in the upper half of the address space. Adding a fixed
  window at e.g. `0xFFFF8000_00000000` for accessing process page tables would
  allow direct modification without CR3 switches.

**Recommended**: Use a kernel virtual address range (approach B). Map each process's
PML4 into a reserved slot at a fixed virtual address. The kernel can then modify any
process's page tables by writing to addresses in this range.

### Where to Start
1. Open `AGENTS.md` and read the "Next Session" section for context
2. Read `kernel_rust/src/paging.rs` — understand `map_4k`, `write_cr3`, `active_l4()`
3. Read `kernel_rust/src/process.rs` — Process struct, spawn_init, sys_fork, load_flat_binary
4. Read `kernel_rust/src/elf.rs` — ELF loader that maps pages
5. Make a plan for the kernel virtual address window

## Task 2: vish Rust Frontend on VIBIX

### Goal
Replace the NASM flat binary shell (`vish.asm`) with a Rust ELF binary compiled from
vish's cross-platform Rust core and a thin VIBIX platform layer.

### Reference: GVIBU Rust ELF
`../gvibu-ai-lab/vibix-lib/` has a working Rust ELF build:
- `Cargo.toml`: targets `x86_64-unknown-none`, `#![no_std]`
- `src/main.rs`: `_start` entry with inline asm for stack + arg registers
- `vibix.ld`: linker script placing `.text` at `0x2000000`
- `src/sys.rs`: syscall wrappers via inline asm

### What vish Needs
The Rust core is in `../vish/src/`:
- `lib.rs` — shared core
- `parse.rs` — command parsing
- `readline.rs` — line editing with history
- `exec.rs` — command execution (fork/exec/waitpid)
- `io.rs` — I/O abstractions
- `builtins/` — built-in commands
- `linux/` — Linux platform backend (reference for a VIBIX backend)

### Implementation Steps
1. Create `src/vibix/mod.rs` in `../vish/` with syscall wrappers:
   ```rust
   pub fn sys_write(fd: u64, buf: *const u8, len: u64) -> u64 {
       let ret: u64;
       unsafe { core::arch::asm!("syscall", in("rax") 1u64, in("rdi") fd,
           in("rsi") buf, in("rdx") len, lateout("rax") ret,
           out("rcx") _, out("r11") _); }
       ret
   }
   ```
2. Copy the GVIBU linker script and Cargo setup
3. The ELF entry point needs inline asm to set RSP (to `0x2006000` for ELF, not `0x2002000`)
4. Build with `cargo build --release --target x86_64-unknown-none`
5. Copy binary to initramfs as `/bin/vish_rust`
6. Test with `make INIT=vibit run` (VIBIT tries `/bin/vish` — point it at the new binary)

### Current vish.asm Backend
`../vish/src/vibix/vish.asm` — the NASM version already has all the shell features
(readline, history, built-ins). The Rust frontend would replace this with compiled code
that's easier to maintain and extend.

## Verification
- `make test` — must pass 15/15 (default init_demo init)
- `make test_vibit` — must pass 7/7 (VIBIT + vish init, checks fork/exec/blocking-IO)
- `make INIT=vibit run` — interactive shell with `vish$ ` prompt
