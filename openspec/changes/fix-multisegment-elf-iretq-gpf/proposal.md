## Why

A historical deepwork report described a multi-segment Rust ELF GPF at `iretq`. That report does not describe the current tree: the Candidate/Active/Retired rewrite is absent, and the committed baseline does not reproduce that GPF. The current `test_vibit_rust` probe now reaches Rust ELF entry after the current-baseline overlap fix.

## Bounded remediation

- Correct DEBUG-only syscall and interrupt return-frame diagnostics so saved user RSP and the frame pointer are labeled and read from the actual post-debug-push offsets.
- Reconcile the investigation artifacts with the current baseline and harness evidence.
- Apply the minimal current-baseline fix after the reproducible shared-page defect was demonstrated: load-local page provenance preserves shared PT_LOAD pages.
- Keep the Rust ELF probe truthful: entry execution, no exception, preserved provenance, and at least three global DEBUG IRQ observations are required for success; those observations do not prove Rust-process round trips.

## Current evidence

- The historical Candidate/Active/Retired paging rewrite is absent and is not reconstructed or modified.
- The committed baseline passes `make test`'s 21 required markers and `make test_vibit`'s 7 markers.
- The pre-fix probe recorded Page Fault #14 at RIP `0x2000000`, raw error `0x5`, and observed CR2 `0x0`; the overlap defect is now fixed and the probe reaches Rust `OK` without exception.
