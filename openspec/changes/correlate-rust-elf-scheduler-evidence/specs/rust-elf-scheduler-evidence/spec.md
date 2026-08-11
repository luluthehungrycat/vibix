## ADDED Requirements

### Requirement: Target identity is explicit and address-space correlated
The VIBIX DEBUG probe SHALL publish exactly one authoritative target record for
the Rust ELF process containing its PID, expected PML4/CR3, entry point, and
one or more executable user RIP intervals derived from the loaded ELF.

#### Scenario: Target registration is complete
- **WHEN** the VIBIX-owned Rust ELF fixture reaches the post-load handoff
- **THEN** the serial evidence contains target PID, expected CR3/PML4, entry, and executable RIP interval metadata

#### Scenario: Target metadata is missing or ambiguous
- **WHEN** no target record exists, or duplicate records disagree on PID, CR3, entry, or ranges
- **THEN** the focused validation rejects the run and does not substitute global IRQ counts as proof

### Requirement: Scheduler events identify target transitions
The VIBIX DEBUG probe SHALL emit compact, parseable `SCHED_OUT` and `SCHED_IN`
records for the target process, including PID, active/expected CR3, user RIP,
CS, RFLAGS, user RSP, and the kernel-frame pointer; `SCHED_OUT` SHALL identify
the selected next PID and `SCHED_IN` SHALL identify the previous PID.

#### Scenario: Target is scheduled out
- **WHEN** the scheduler captures a target frame and selects another process
- **THEN** one `SCHED_OUT` record identifies the target PID, non-target next PID, target CR3, and saved user-frame values

#### Scenario: Target is scheduled in
- **WHEN** the scheduler selects the target and writes its address space before the return path
- **THEN** one `SCHED_IN` record identifies the target PID, non-target previous PID, matching active/expected CR3, and restored frame values

#### Scenario: Same-process selection is not a round trip
- **WHEN** a scheduler invocation selects the target PID again without another process intervening
- **THEN** the harness does not count that event as a target scheduler round trip

### Requirement: User-mode return is distinguished from frame preparation
The VIBIX DEBUG probe SHALL emit a `USER_RETURN` record only after a target
process has actually returned to user mode, as established by a subsequent
interrupt entry with the target PID, matching active CR3, user-mode CS/SS, and
an interrupted RIP inside the declared executable interval.

#### Scenario: Target reaches user mode after schedule-in
- **WHEN** a target `SCHED_IN` is followed by an interrupt taken from target user code
- **THEN** the evidence contains `USER_RETURN` with matching PID/CR3, user CS/SS, IF-enabled RFLAGS, valid user RSP, and in-range RIP

#### Scenario: Prepared frame never executes
- **WHEN** a target `SCHED_IN` is observed but no qualifying user-mode interrupt follows
- **THEN** the harness does not treat the pre-`iretq` frame as a user-mode return

### Requirement: Bounded correlated round-trip success gate
The VIBIX-owned harness SHALL require a named configured threshold of at least
three complete target-process round trips and SHALL pass only when the ordered
event stream proves that threshold, with no exception or panic.

#### Scenario: Required round trips complete
- **WHEN** the stream contains `SCHED_IN(target)`, `USER_RETURN(target)`, `SCHED_OUT(target, next != target)`, and a subsequent `SCHED_IN(target, previous != target)` for the configured threshold
- **THEN** the focused Rust ELF validation passes the scheduling-evidence gate and reports the exact threshold and observed count

#### Scenario: Correlation invariant fails
- **WHEN** any counted event has a mismatched PID, CR3/PML4, out-of-range RIP, invalid user frame, or exception/panic marker
- **THEN** the focused validation fails even if the global IRQ count is high

#### Scenario: Bounded probe cannot reach completion
- **WHEN** readiness or the configured event threshold is not observed before the current harness timeout and retry policy is exhausted
- **THEN** the result is reported as `BLOCKED` or `FAIL` with the exact accelerator, attempt, transcript, and missing event; it is not reported as success

### Requirement: DEBUG isolation and regression preservation
The correlated evidence SHALL be DEBUG-gated, low-volume, and additive to the
existing Rust ELF/provenance assertions; DEBUG-off behavior and existing default
and VIBIT validation contracts SHALL remain unchanged.

#### Scenario: Release build runs without probe logging
- **WHEN** the kernel is built without the DEBUG feature
- **THEN** no correlated scheduler records are emitted and existing release behavior remains unchanged

#### Scenario: Existing validation remains green
- **WHEN** the new focused evidence gate is implemented
- **THEN** the current default, VIBIT, TTY/SIGINT, ELF rollback, Rust ELF, and other currently available harness targets retain their existing assertions unless a target is explicitly blocked by its own prerequisite

### Requirement: Evidence limitations and false positives are documented
The change documentation and harness report SHALL state that global IRQ counts,
PID-only records, pre-return frames, stale serial output, and broad user address
ranges are insufficient evidence, and SHALL report limitations such as DEBUG
serial timing perturbation and lack of general scheduler fairness proof.

#### Scenario: Report explains the evidence boundary
- **WHEN** validation completes or is blocked
- **THEN** the report includes the configured threshold, exact correlated count, accelerator/attempt, exception status, and any limitation or blocker

#### Scenario: Weak evidence is rejected
- **WHEN** only global IRQ lines or an uncorrelated target PID are present
- **THEN** the scheduling-evidence gate remains unsatisfied
