## Status

The historical GPF scenario is not proven on the current tree. The Candidate/Active/Retired rewrite is absent and remains out of scope. The current-baseline PT_LOAD overlap defect is fixed by load-local page provenance; Rust entry output now proves the entry page and later segment bytes survive. PID/range-correlated Rust-process scheduling evidence remains blocked.

## Requirements under investigation

### Requirement: Multi-segment ELF scheduling stability
The kernel SHALL schedule an ELF64 binary with multiple PT_LOAD segments without a GPF or page fault.

### Evidence gate
A current-baseline probe may pass when it observes Rust ELF entry output, no exception, preserved
shared-page provenance, and at least three DEBUG global IRQ observations. These IRQ lines are not Rust-process scheduling evidence because
the current serial format has no PID or instruction-range correlation. The historical pre-fix blocker was Page Fault #14 at RIP `0x2000000` with raw error `0x5` and
observed CR2 `0x0`; the current fixed probe reports no exception. Global IRQ observations do
not establish process scheduling correlation.

### Requirement: Existing harness remains green
The current harness contains 21 required default markers and 7 VIBIT markers. `make test` and `make test_vibit` must preserve those exact marker sets.

### Requirement: DEBUG frame diagnostics
DEBUG-only diagnostics SHALL report the actual iretq frame fields and distinguish saved user RSP from the frame pointer.
