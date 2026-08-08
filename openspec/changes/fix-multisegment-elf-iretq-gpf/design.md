## Current baseline and scope

The historical Candidate/Active/Retired paging rewrite is absent from this tree. It is not reconstructed, inferred, discarded, or modified by this pass. The committed baseline uses the existing per-process PML4 implementation.

The historical multi-segment ELF GPF is not reproducible here because its Candidate/Active/Retired
rewrite is absent. Phase 2 instead confirmed a current-baseline PT_LOAD overlap defect; Phase 3
fixed that defect with the Oracle-approved minimal loader change. The historical GPF remains
unconfirmed and out of scope.

## Diagnostics

After GPR restoration, the interrupt path skips `int_no` and `err_code` before its DEBUG block, so four pushes place RIP at `+32`, CS at `+40`, RFLAGS at `+48`, saved user RSP at `+56`, and SS at `+64`. The syscall path skips those fields only after its DEBUG block, so its corresponding offsets are RIP `+48`, CS `+56`, RFLAGS `+64`, saved user RSP `+72`, and SS `+80`. Diagnostics label saved user RSP separately from the frame pointer.

## ELF loader assessment

`elf.rs` now keeps a bounded linear `LoadedPage` provenance table local to one load call. The first encounter of a virtual page allocates, maps, and zeroes it; later PT_LOADs reuse only the recorded physical frame. Pre-existing fork/old-image mappings are never reused. Exact partial-page copy ranges remain ordered by program-header order, so later overlapping file bytes win without re-zeroing. `translate_in_pml4` remains a general walker and is not used by the loader.

## Probe evidence gate

`test_vibit_rust` passes when it observes VIBIT markers, Rust entry output (`OK`), no exception,
preserved shared-page provenance, and at least three DEBUG global `IRQ frame:` observations.
Those lines are not claimed as Rust-process round trips because they lack PID/range correlation.
The pre-fix Page Fault #14 had raw error `0x5` and observed CR2 `0x0`; it is historical evidence
for the fixed overlap defect, not the current probe status.

## Phase 2/3 closure evidence

Phase 2 proved that PT_LOAD[1] remapped the page populated by PT_LOAD[0], producing Page Fault
#14 at RIP `0x2000000` with raw error `0x5`, observed CR2 `0x0`, and zero entry bytes. Phase 3
moved page provenance outside the PT_LOAD loop into a bounded load-local linear table. The fixed
probe preserves frame `0x29c000`, entry bytes, and the second segment's `OK\n` bytes; Rust entry
runs without exception. No CR3 redesign, permission/NX change, rollback, dynamic linking, or
external-repository change was made.

## Non-goals

- Reconstructing or fixing the absent historical rewrite or historical GPF.
- Claiming Rust-process scheduling correlation from global IRQ output.
- Changing page-table architecture, NX policy, or external repositories.
