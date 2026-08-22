# Next Session

## Current repository status

The historical Candidate/Active/Retired paging rewrite is absent from this
tree. Do not reconstruct, infer, discard, or modify it from historical notes.
The current baseline uses the existing per-process PML4 implementation.

The harness has 25 required default markers (`test_kernel.py::CHECKLIST`) and
the VIBIT integration uses external VIBIT with the NASM vish shell. The default
suite passed all required markers, and the VIBIT integration passed fork/exec,
blocking-read, reaping, shell continuation, and prompt checks.

## Current Rust ELF scheduler evidence

The focused Rust ELF probe reaches entry output `OK`, preserves the shared
entry-page frame across both PT_LOADs, and reports no exception. The actual
bounded command was:

```text
timeout 240s make test_vibit_rust
```

It exited 0. The prerequisite `../vish` existed and `make -C ../vish elf` was
already up to date. KVM attempts 1 and 2 reached serial readiness but timed out
without completion and were classified `FAIL`. TCG attempt 1 reached readiness
and completion and was selected `PASS` with reason `completion predicate
observed`. The selected evidence was PID 3, CR3 `0x297000`, executable
interval `[0x2000000,0x2000019)`, 3 correlated `IN`/`USER`/`OUT` round trips,
and 10 events. The selected transcript contained Rust `OK`, preserved frame
`0x2a3000`, and no exception. This is a runtime `PASS`, not `BLOCKED`.

The actual large-lifecycle command was:

```text
timeout 240s make test_vibit_rust_large
```

It exited 0. KVM attempt 1 reached readiness without completion and was
classified `FAIL`; TCG attempt 1 was selected `PASS`. The 257-page synthetic
ELF produced PID 3 / CR3 `0x297000` scheduler evidence with 3 round trips and
10 events, exact interval `[0x2000000,0x2122000)`,
`LARGE_ELF_OK`, VIBIT reap/respawn, the second completion marker, no exception,
and byte-for-byte initramfs restoration.

The ordinary integration command was:

```text
timeout 180s make test_vibit
```

It exited 0. KVM attempt 1 was selected `PASS`; VIBIT banner, init, shell
spawn, reaper, shell start, vish exec, blocking prompt, and continuation
markers all passed.

## Validation record

These commands were run by the fixer and exited 0:

- `timeout 300s bash -c 'make clean && make && make test'` — default build and
  all 25 required checks passed.
- `timeout 240s make test_elf_rollback` — ELF, fork, scratch, flat, stack, and
  final rollback markers passed.
- `timeout 240s make test_tty_sigint` — all deterministic TTY ownership and
  no-exception assertions passed. Its bounded shell probe selected KVM with
  readiness but no product completion marker and therefore correctly reported
  `FAIL`; the focused marker test still passed.
- `timeout 240s bash -c 'make clean && make DEBUG=1'` — passed with existing
  Rust/NASM/linker warnings.
- `python3 -m py_compile test_kernel.py` — passed.
- `python3 anti_cheat.py` — passed.
- `openspec status --change close-openspec-review-blockers` — 4/4 artifacts
  complete.
- `openspec validate --all --strict --no-interactive` — 9 passed, 0 failed.
- `git diff --check` — passed.

Acceptance tasks 8.3 and 8.4 and adversarial review tasks 9.1–9.3 are
evidenced and complete. The final 2026-08-22 review found no source-level
blockers. Delivery tasks 10.1–10.4 remain pending for the orchestrator. No
sibling source was changed, and no commit, push, or PR was performed.

## Descriptor error semantics

The VIBIX fd follow-up is complete. Covered close/dup/dup2 and VFS descriptor handlers reject
out-of-range, closed, and stale descriptors with the documented VIBIX results. `dup2` validates
the source before same-fd no-op handling, preserves same-OFT refcounts, and DEBUG builds assert
fd-table/OFT invariants. The flat fixture covers legacy IDs 1/2 sentinels, VFS IDs 13–20 and
24–26, alias closure, and pipe survival.

## Diagnostics

After four DEBUG pushes, `kernel/interrupts.asm` has already skipped `int_no` + `err_code`, so its
iret fields are RIP `+32`, CS `+40`, RFLAGS `+48`, saved user RSP `+56`, SS `+64`. The syscall
return path skips those fields afterward, so its fields are RIP `+48`, CS `+56`, RFLAGS `+64`,
saved user RSP `+72`, SS `+80`. `FRAME` is the iretq frame pointer; `USER_RSP` is the saved user stack value.

## Remaining actionable work

1. Correlated DEBUG evidence does not claim scheduler fairness or general
   multi-process liveness; extend that separately only if needed.
2. Successful old-root reclamation after address-space replacement remains a
   documented follow-up ownership/reference-counting change.
3. The large-ELF fixture remains test-time/temporary and ignored.
4. The ordinary marker suite is not a DEBUG-image gate because DEBUG rollback
   self-tests can delay normal userspace markers; dedicated DEBUG TTY and
   rollback harnesses pass their assertions.
5. The archived `fix-multisegment-elf-iretq-gpf` change is outside the active
   aggregate and is not a current blocker. The historical Candidate/Active/
   Retired rewrite remains out of scope.
