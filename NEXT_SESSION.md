# Next Session

## Repository State

Branch: `scheduler-phase1` → PR [#6](https://github.com/luluthehungrycat/vibix/pull/6) to `main`
All commits pushed. Test: `make test` → 15/15 passes. `make test_vibit` → 7/7 passes.

## What's Implemented (Per-Process Page Tables, Phases 1-3)

- **`paging.rs`**: `create_pml4()` creates per-process PML4 with per-process PDPT, isolated user PD entries
  (user PD entries zeroed unless `copy_user=true` for fork), deep-copies PT entries for user pages to
  prevent parent/child sharing. `map_4k_target()` maps into a target process's PML4 via temporary CR3 switch.
  `translate_in_pml4(vaddr, pml4_phys)` walks a specific PML4 (vs `translate()` which uses current CR3).
- **`process.rs`**: Every `Process` has `pml4_phys: u64`. `spawn_init()` creates per-process PML4s for
  PID 1 and idle. `sys_fork()` creates child PML4 via `create_pml4(pmm, true)`. `sys_exec()` maps
  pages into the exec'd process's own PML4. `start_scheduler()` switches CR3 before first context switch.
  `scheduler_tick()` and `scheduler_switch_exit()` switch CR3 via `write_cr3(next.pml4_phys)`.
- **`elf.rs`**: Pages zeroed via virtual address (in target process's PML4) rather than physical address
  (identity map). Overlapping segments detected via `translate_in_pml4` on the target PML4.
  Relaxation disabled (all pages stay RW — no NX bit anyway).

## Remaining Issue: GPF #13 on Rust ELF context switch (Task 2)

### Symptom
When VIBIT fork-execs the Rust ELF `/bin/vish_rust` (a multi-segment ELF with .text + .rodata),
the child process executes and prints OK, then a **GPF #13** occurs at kernel address `0x20001E8`
(the `iretq` instruction in `irq_common` in `kernel/interrupts.asm`). Error code `0x18` indicates SS
selector mismatch: the target frame has `CS=0x08` (kernel) and `SS=0x1B` (user).

### What we know
- **NASM flat binary** works perfectly (VIBIT + vish.asm passes 7/7)
- **Single-segment ELF** (text only, e.g. just `loop {}` in `main()`) works perfectly
- **Multi-segment ELF** (text + rodata, e.g. `main()` with `b"OK\n"` string) prints output then GPFs
- DBG trace in `scheduler_tick` confirmed the frame has **correct CS=0x23** moments before `iretq`
- The corruption happens **between** `scheduler_tick` returning `next.kernel_rsp` and the assembly `iretq`
- `write_cr3(next.pml4_phys)` in `scheduler_tick` switches page tables before the return
- The `map_4k` / `map_4k_target` sequence during ELF loading adds `PAGE_USER` to PML4[0] and PDPT[0]
  via `(*l4)[l4i] |= PAGE_USER`, which modifies the shared identity map's intermediate entries

### Probable root cause
The ELF loader's `map_4k` call adds `PAGE_USER` to PML4[0] and PDPT[0] when mapping user pages.
In the target process's PML4 (created by `create_pml4` from the boot PML4), these entries may NOT have
had `PAGE_USER` before (the boot PML4 flags depend on boot.asm). Adding `PAGE_USER` changes how the
CPU's page walker traverses the identity map entries. When the scheduler later switches CR3 to VIBIT's
PML4 and iretqs, the TLHas been flushed but the PML4/PDPT entries have different flags than what
`get_or_create_table` would have set during VIBIT's own page walks. This mismatch causes the iretq
to read a stale/wrong physical page for the frame.

A more targeted fix would either:
1. **Skip setting `PAGE_USER` on l4[l4i] and l3_ptr[l3i] for `l4i == 0 && l3i == 0`** in `map_4k()`
   when called from the ELF loader path (the PML4[0]/PDPT[0] entries should retain their boot-time flags)
2. **Save and restore PML4[0] and PDPT[0] around the ELF load** (tried — broke user-mode page walks
   because removing PAGE_USER blocks user access through those intermediate entries)
3. **Avoid using `map_4k` for multi-segment ELFs entirely** — map pages with raw PT manipulation
   that doesn't touch intermediate entry flags
4. **Add assembly-level check in `irq_common`** right before `iretq` to dump CS/SS via Bochs debug
   port (0xE9) and confirm the frame values at the exact iretq moment

### Files to focus on
- `kernel/interrupts.asm` — `irq_common` handler, the `iretq` at ~line 184
- `kernel_rust/src/paging.rs` — `map_4k()` (line 234), `get_or_create_table()` (line 68) — these add `|= PAGE_USER`
- `kernel_rust/src/elf.rs` — ELF loader that triggers the issue via `map_4k_target`
- `kernel_rust/src/process.rs` — `scheduler_tick()` (line 462), `scheduler_switch_exit()` (line 507)
- `kernel/syscall_entry.asm` — `.exit_or_block` frame building

### Verification
```bash
make test        # Must pass 15/15
make test_vibit   # Must pass 7/7
make INIT=vibit run  # For manual testing with Rust ELF (set /bin/vish = vish_rust)
```

## vish Rust frontend status

The vish Rust ELF binary is at `../vish/src/bin/vibix.rs` and gets built to
`../vish/target/x86_64-unknown-none/release/vibix`. The initramfs has `/bin/vish` (NASM) and
`/bin/vish_rust` (Rust ELF). The Rust entry point uses `#[unsafe(naked)]` with `naked_asm!` 
that calls `sym main` — this pattern works for single-segment ELFs but triggers the GPF for
multi-segment ELFs. The `../vish/src/vibix/mod.rs` file has the full VIBIX backend: syscall
wrappers, global allocator via `sys_brk`, `VibixStdin`/`VibixStdout` I/O, and a `run_repl()` entry point.
