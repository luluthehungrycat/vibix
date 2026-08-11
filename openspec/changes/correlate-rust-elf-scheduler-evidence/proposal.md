## Why

`test_vibit_rust` currently proves Rust ELF entry output, no exception, shared-page provenance, and only a global IRQ count. It does not prove that the Rust ELF process is scheduled in, scheduled out, and returned to user mode across multiple scheduler round trips, so the highest-priority remaining evidence gap is still open.

## What Changes

- Add a VIBIX-owned DEBUG evidence protocol that identifies the target Rust ELF PID and correlates scheduler/IRQ events with its user instruction range and address-space identity (CR3).
- Distinguish target-process scheduled-in, scheduled-out, and user-mode-return observations, including the saved user frame invariants needed to connect them.
- Extend the VIBIX-owned Rust ELF fixture and bounded harness assertions to require a configured number of target-process round trips, no exception/panic, and consistent PID/RIP/CR3/frame evidence.
- Keep logging DEBUG-gated and low-volume enough to minimize timing perturbation; preserve DEBUG-off behavior and all existing release/VIBIT tests.
- Document evidence limitations, rejected false-positive interpretations, bounded timeouts, and cases that must remain `BLOCKED` rather than being inferred as success.

## Capabilities

### New Capabilities

- `rust-elf-scheduler-evidence`: Correlated, bounded proof that a VIBIX Rust ELF process survives multiple scheduler round trips and returns to user mode with consistent PID, RIP-range, CR3, and frame evidence.

### Modified Capabilities

- None.

## Impact

- VIBIX kernel DEBUG-only scheduler/interrupt diagnostics and the Rust ELF test fixture/harness may change.
- `test_vibit_rust` assertions and its serial evidence parser will gain PID/range/CR3 correlation without changing the production DEBUG-off path.
- Existing `make test`, `make test_vibit`, ELF provenance checks, and sibling-repository ownership boundaries remain in force.
- No VIBIT, vish, or GVIBU source changes are in scope; no sibling build output is required for the VIBIX-owned fixture beyond existing test prerequisites.
