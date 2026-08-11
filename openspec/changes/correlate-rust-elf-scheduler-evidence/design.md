## Context

The current Rust ELF probe reaches `OK`, proves the shared entry-page frame,
and reports no exception, but its `IRQ frame:` count is global. VIBIX already
owns the process table, PID selection, saved user frame, per-process PML4, CR3
switch, and DEBUG-gated scheduler diagnostics. The missing link is a compact,
machine-checkable event stream that connects those values to the ELF process
under test.

The change is VIBIX-only. The existing sibling Rust ELF may remain an input to
the probe, but no sibling source, build logic, or ABI is changed. All new
fixture orchestration, parsing, and acceptance logic belongs in VIBIX.

## Goals / Non-Goals

**Goals:**

- Identify one ELF target by PID, PML4/CR3, entry, and a load-derived user RIP
  interval.
- Emit and parse bounded `SCHED_OUT`, `SCHED_IN`, and `USER_RETURN` events for
  that target, with saved-frame fields sufficient to verify the address-space
  and user-return invariants.
- Require a named, configurable number of complete target round trips (default
  three), no exception or panic, and ordered event evidence in bounded QEMU.
- Keep the new records behind the existing DEBUG feature and use compact output
  rather than the current full-frame dump in the hot path.
- Preserve the current entry/provenance assertions and all DEBUG-off behavior.

**Non-Goals:**

- Changing scheduler policy, process lifetime semantics, CR3 architecture, ELF
  loading, or the sibling VIBIT/vish/GVIBU repositories.
- Claiming fairness, wall-clock timing, or PID/range correlation for processes
  other than the explicitly registered target.
- Treating global IRQ counts, a target PID alone, or repeated serial text as
  scheduler proof.

## Decisions

### 1. Register target identity at the VIBIX ELF handoff

When the VIBIX ELF exec path has the final entry and loaded-page range, it will
publish one DEBUG-only target record containing `pid`, `pml4`/`cr3`, `entry`,
`rip_lo`, and `rip_hi`. The range is the union of the executable PT_LOAD
virtual intervals used by the fixture; endpoints are explicit and page-safe.
The harness rejects missing, duplicate, or internally inconsistent target
metadata.

**Alternative rejected:** infer the PID from the first `DBG EXEC` line or infer
the RIP interval from a global constant. Those approaches can select VIBIT,
the idle process, or a stale ELF and create false positives.

### 2. Use three semantic event points

The DEBUG event protocol is a compact, parseable line format with an event kind
and all correlation fields on the same logical record:

- `SCHED_OUT`: emitted when a target's current saved frame is captured for a
  scheduler decision. It includes the next PID, saved RIP/CS/RFLAGS/user-RSP,
  kernel-frame pointer, and the target CR3.
- `SCHED_IN`: emitted after the selected target's CR3 has been written and its
  saved frame has been selected, before the assembly return path. It includes
  the previous PID, restored RIP/CS/RFLAGS/user-RSP, kernel-frame pointer, and
  active/expected CR3 values.
- `USER_RETURN`: emitted at a later interrupt entry only when the current PID
  is the target, the active CR3 matches the target PML4, and the CPU-saved CS
  proves the interrupted code was in user mode. It includes the interrupted
  user RIP and frame values. This event is evidence that execution actually
  crossed the `iretq` return boundary; a pre-`iretq` `SCHED_IN` alone is not.

The event sites cover both timer-driven `scheduler_tick` and syscall-driven
`scheduler_switch_exit`. `SCHED_OUT` must record a different next PID for a
round trip to count, preventing a scheduler call that selects the same process
from inflating the result.

**Alternative rejected:** add PID text to every raw assembly IRQ dump. That
would greatly increase serial traffic and still would not identify the active
CR3 or prove the subsequent user-mode return.

### 3. Correlate with an ordered finite-state parser

The VIBIX harness will parse only the new structured records and maintain a
target state machine. A completed round trip requires:

```text
SCHED_IN(target) → USER_RETURN(target) → SCHED_OUT(target, next != target)
  → SCHED_IN(target, previous != target)
```

The final `SCHED_IN` starts the next cycle and may be followed by another
`USER_RETURN`. Every target event must satisfy `cr3 == pml4`, RIP inside the
declared executable interval, canonical/nonzero user RSP where applicable,
user CS/SS and IF invariants where applicable, and matching frame metadata.
The parser reports the configured threshold and the exact completed count;
global `IRQ frame:` lines cannot satisfy it.

**Alternative rejected:** count `SCHED_IN` or IRQ records independently. Counts
without ordering, a different PID, a different CR3, or a user-range check can
pass while the target never returns to user mode.

### 4. Keep logging low-volume and DEBUG-only

The new records are emitted only for the registered target and only at the
three state transitions. They use one compact record per event and avoid the
existing per-register frame dump for the new assertion. DEBUG-off builds do not
allocate, format, or emit this evidence. The harness uses readiness/completion
markers and bounded KVM-then-TCG attempts rather than sleeps or an unbounded
QEMU session.

### 5. Preserve existing probe gates and use a VIBIX-owned fixture

`test_vibit_rust` keeps its existing Rust `OK`, no-exception, entry-page
provenance, and ELF handoff checks, then adds the correlated event gate. The
fixture/orchestration and parser remain in VIBIX; a sibling Rust ELF can be
built as the existing immutable prerequisite, but the proof does not require
source changes in adjacent repositories. If the target metadata or event stream
is absent, the test fails or is explicitly blocked on its prerequisite rather
than falling back to global IRQ evidence.

## Risks / Trade-offs

- [Serial output changes scheduling timing] → Emit only target events, use a
  compact DEBUG record, keep the old verbose diagnostics out of the new hot
  path, and retain bounded retries. Report that DEBUG timing is not a release
  performance claim.
- [A stale target record is paired with a later process] → Clear target state
  per fresh QEMU run, require one target registration, match PID and CR3 on
  every event, and reject duplicate/conflicting metadata.
- [A pre-return frame is mistaken for user execution] → Count a round trip only
  after `USER_RETURN` is observed from a CPU-saved user-mode interrupt frame.
- [A RIP range is too broad] → Derive explicit executable PT_LOAD intervals
  from the current ELF load and reject events outside the union; do not use the
  whole user address space.
- [TCG/KVM or serial readiness differs] → Use fresh bounded attempts, readiness
  markers, KVM-to-TCG fallback, and retain the strongest transcript with the
  exact accelerator/attempt in the report.
- [The Rust process exits before reaching the threshold] → The VIBIX-owned
  fixture must keep the target alive long enough for the bounded threshold; an
  absent prerequisite or unavailable fixture is `BLOCKED`, never fabricated
  scheduling success.

## Migration Plan

1. Add the DEBUG-only target/event protocol and VIBIX harness parser behind the
   current test gate.
2. Run focused Rust ELF validation with the configured threshold, then run the
   existing release, DEBUG, default, and VIBIT gates.
3. Rollback is removal/disablement of the new DEBUG event gate and fixture
   assertions; DEBUG-off binaries and existing marker contracts remain the
   compatibility boundary.

## Open Questions

- Should the target RIP interval be emitted by the ELF loader as a single union
  or serialized as multiple executable PT_LOAD intervals? The implementation
  must preserve exact interval membership either way.
- Which existing DEBUG target marker is the most stable handoff point for
  registering the PID without increasing serial volume? The implementation
  should choose one authoritative record and reject ambiguous streams.
- What bounded QEMU timeout and retry count best fit the current harness after
  compact logging is added? Use measured current harness behavior, not a
  historical constant.
