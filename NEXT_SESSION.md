# Next Session

## Current repository status

The historical Candidate/Active/Retired paging rewrite is absent from this tree. Do not reconstruct, infer, discard, or modify it from historical notes. The committed baseline uses the existing per-process PML4 implementation.

The current harness contains 21 required default markers (`test_kernel.py::CHECKLIST`) and 7 VIBIT markers. The committed baseline passes `make test` and `make test_vibit` when built with their respective targets.
The VIBIT integration uses the external VIBIT init with the NASM vish shell and passes the bounded fork/exec, blocking-read, reaping, and shell-continuation checks.

## Current Rust ELF status

Phase 3 applied the Oracle-approved current-baseline overlap-loader fix. `elf::load` now uses a
bounded load-local linear `LoadedPage` provenance table: each virtual page is allocated, mapped,
and zeroed once per load; later PT_LOADs reuse only recorded frames. Existing fork/old-image
mappings remain excluded. Exact partial-page copy order is preserved, with later file bytes
winning and no permission/NX changes.

The focused Rust ELF probe reaches entry output `OK`, preserves the entry-page frame across both
PT_LOADs, and reports no exception. `test_vibit_rust` now validates three bounded, ordered
PID/CR3/RIP-correlated scheduler round trips using `IN`, `USER`, and `OUT` DEBUG records. Final
evidence was PID 3, CR3 `0x297000`, executable RIP range `0x2000000–0x2000019`, and 10 correlated
events. The historical Candidate/Active/Retired rewrite and historical GPF remain out of scope.

The full final validation matrix passed: clean release/debug builds, default/VIBIT boot tests,
TTY SIGINT, Rust ELF, large synthetic ELF, ELF rollback, anti-cheat, and strict OpenSpec validation
for the scheduler-evidence change plus the three related follow-up changes. Generated build and
fixture artifacts were cleaned afterward; no sibling source was changed.

## Diagnostics

After four DEBUG pushes, `kernel/interrupts.asm` has already skipped `int_no` + `err_code`, so its
iret fields are RIP `+32`, CS `+40`, RFLAGS `+48`, saved user RSP `+56`, SS `+64`. The syscall
return path skips those fields afterward, so its fields are RIP `+48`, CS `+56`, RFLAGS `+64`,
saved user RSP `+72`, SS `+80`. `FRAME` is the iretq frame pointer; `USER_RSP` is the saved user stack value.

## Remaining actionable work

1. Correlated DEBUG evidence does not claim scheduler fairness or general multi-process liveness;
   extend that separately only if needed.
2. If separately scoped, evaluate non-atomic exec/OOM behavior or additional ELF overlap
   semantics. Do not reopen the fixed shared-page provenance diagnosis.
3. Consider the absent Candidate/Active/Retired rewrite only if its actual artifact is supplied;
   it remains out of scope in the current tree.
