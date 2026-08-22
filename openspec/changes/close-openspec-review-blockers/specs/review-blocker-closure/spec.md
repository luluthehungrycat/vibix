<!-- Status: implementation, acceptance validation, and adversarial review are
complete; delivery remains pending. Runtime evidence and exact command results
are recorded in CHANGELOG.md and NEXT_SESSION.md. -->

## ADDED Requirements

### Requirement: ELF exec failure is transactional

An ELF `exec()` attempt SHALL commit the new image only after all image mappings, stack pages, process metadata, and syscall return state have succeeded. If any allocation or mapping step fails, the kernel MUST release or restore every resource owned by the attempt, invalidate affected translations, and leave the prior process image and allocator state usable.

#### Scenario: Stack allocation fails after ELF mappings
- **WHEN** ELF segments have been mapped and at least one new user stack page has been allocated, but a later stack allocation fails
- **THEN** the attempted mappings and allocations are rolled back, no partial new stack remains reachable, and the process retains a valid pre-exec image or a defined error return without corrupting allocator state

#### Scenario: Loader allocation fails before commit
- **WHEN** ELF page, page-table metadata, or provenance allocation fails during loading
- **THEN** all resources created by that attempt are reclaimed, affected TLB entries are invalidated or the address space is restored, and the failure is reported as the documented VIBIX error

### Requirement: Fork user leaves are eagerly isolated

Fork SHALL allocate and copy every present non-huge user 4 KiB leaf at or above
`USER_CODE_ADDR` into distinct child physical frames, preserving PTE flags and
without copy-on-write. User huge mappings SHALL be rejected safely. Partial
failure SHALL free only child-owned leaves and page-table frames.

#### Scenario: Fork copies writable user leaves
- **WHEN** the parent has two present user 4 KiB leaves and fork succeeds
- **THEN** the child PTEs resolve to distinct physical frames, and a child write does not change parent bytes

#### Scenario: Fork cloning fails after one leaf
- **WHEN** an injected PMM failure occurs after at least one child leaf was copied
- **THEN** the child construction is unpublished and cleaned up, parent frames/mappings remain unchanged, and subsequent allocator reuse is deterministic

#### Scenario: Fork copies through the reserved scratch page
- **WHEN** fork clones a present user leaf, including a frame that is not safely dereferenceable through a direct physical address
- **THEN** the parent virtual address is read and the child frame is temporarily mapped through the reserved kernel-only scratch page while interrupts remain disabled across map/copy/unmap; scratch page-table branches are provisioned transactionally and the scratch range is rejected from user mappings

#### Scenario: Fork scratch rollback owns only new branches
- **WHEN** a fork clone fails after scratch PD/PT creation or after one child leaf has been copied
- **THEN** the scratch leaf and empty newly created branches are removed, every child-owned frame is reclaimed, and pre-existing parent page tables and mappings remain allocated and unchanged

### Requirement: Flat exec uses a fresh transaction-owned root

Flat execution SHALL use a fallible fresh-root builder with checked size/page
arithmetic, a supported user-range cap, and a stack page after all code pages.
Root, entry, and RSP publication SHALL occur only after all mappings succeed.
On failure the old CR3/process/syscall state SHALL remain untouched; successful
old-root reclamation is a documented follow-up, not an acceptance claim here.

#### Scenario: Flat allocation or mapping fails
- **WHEN** a flat image fails after at least one code page allocation
- **THEN** only the new root and its owned frames are destroyed, with the old active root and process return state unchanged

#### Scenario: Flat image exceeds the supported range
- **WHEN** checked code-plus-stack arithmetic exceeds the configured user-range cap
- **THEN** the builder rejects the image before mapping or publishing any new root

#### Scenario: Small flat image has a zero-filled tail
- **WHEN** a mapped code page has no remaining image bytes to copy
- **THEN** the loader zeroes the newly allocated page before skipping source-pointer arithmetic and copying, leaving the page tail zero-filled even when PMM data was previously non-zero

### Requirement: Stack rollback has externally observable proof

The stack rollback fixture SHALL use the shared image builder and inject failure
after exactly one stack page. It MUST prove unchanged active-root state,
non-publication, and deterministic allocator reuse after cleanup, without
walking a destroyed root. The final rollback marker SHALL be emitted only after
all rollback cases pass.

#### Scenario: Second stack page fails
- **WHEN** the shared builder maps one stack page and the next allocation is injected to fail
- **THEN** it reports one mapped stack page, preserves the old active root/mapping, and returns the same allocator frames on reuse

### Requirement: Executable evidence preserves exact intervals

DEBUG scheduler evidence SHALL record the complete set of disjoint half-open executable PT_LOAD intervals. The validator MUST reject malformed, overlapping, or incomplete interval records and MUST accept a RIP only when it lies within one recorded interval.

#### Scenario: Executable segments contain an unmapped gap
- **WHEN** an ELF has two executable PT_LOAD ranges separated by an unmapped gap
- **THEN** evidence contains two exact intervals and a RIP in the gap is rejected rather than accepted by a bounding min/max range

#### Scenario: Entry and scheduler RIPs are valid
- **WHEN** the entry and every observed scheduler RIP fall within recorded executable intervals
- **THEN** interval, PID, CR3, sequence, and frame checks can pass without inventing coverage between segments

### Requirement: Large-ELF lifecycle requires scheduler proof

The synthetic large-ELF lifecycle test SHALL require correlated scheduler evidence in addition to its ordered marker, reaper/respawn, continuation, and no-exception checks.

#### Scenario: Lifecycle markers pass without scheduler records
- **WHEN** the fixture emits the first marker, VIBIT emits the reaper/respawn marker, and the continuation marker appears, but required scheduler evidence is absent or invalid
- **THEN** the test MUST fail and MUST report the missing or invalid scheduler evidence

#### Scenario: Complete large-ELF lifecycle is observed
- **WHEN** exact scheduler evidence, the first fixture marker, reaper/respawn marker, continuation marker, and no-exception condition are all observed in the required order
- **THEN** the test passes and reports the complete evidence chain

### Requirement: Root and CI Rust validation use stable Rust

The root Makefile and CI workflow SHALL select the same stable Rust toolchain and `x86_64-unknown-none` target for direct kernel builds and canonical root validation.

#### Scenario: Root build is invoked
- **WHEN** a developer or CI runs the root Makefile kernel build
- **THEN** Cargo is invoked through the explicitly selected stable toolchain with the kernel target and required code-model/feature flags

#### Scenario: CI validates the repository
- **WHEN** the Rust workflow runs
- **THEN** it installs/asserts stable Rust and the target, runs direct kernel checks, and runs the canonical root build/test gates without falling back to an unspecified host toolchain

### Requirement: VIBIT/vish probe failures are deterministic

The VIBIT/vish harness SHALL use bounded readiness, prompt, and completion markers with deterministic failure reasons. A missing required prompt or marker MUST be reported as a failure or environment block according to the tri-state probe contract, not inferred from timeout timing alone.

#### Scenario: Required prompt never appears
- **WHEN** the QEMU process is ready but the required VIBIT/vish prompt or completion marker does not appear before the bounded deadline
- **THEN** the harness reports a named missing-marker outcome with the selected transcript and does not pass the test

#### Scenario: Accelerator cannot establish readiness
- **WHEN** an accelerator is unavailable or cannot produce a usable serial transcript
- **THEN** the harness reports `BLOCKED` with accelerator, attempt, and reason details

#### Scenario: VIBIT probe reaches readiness without completion
- **WHEN** VIBIT serial output is readable but a required boot or shell marker is missing before the bounded deadline
- **THEN** the harness reports `FAIL` with accelerator, attempt, readiness/completion state, selected transcript, and a deterministic missing-marker reason

### Requirement: FD fixtures obey the syscall clobber ABI

The FD regression fixtures SHALL preserve every value live across a syscall according to the documented full-register-clobber ABI, and the documentation SHALL state the tested contract unambiguously where clarification is needed.

#### Scenario: FD value is reused after a syscall
- **WHEN** a fixture needs a descriptor, pointer, length, or comparison value after a syscall
- **THEN** it restores or reloads that value before use and the regression result is independent of caller-saved register contents

#### Scenario: FD regression reaches its assertions
- **WHEN** close, dup, dup2, pipe, read, and write fixture cases execute
- **THEN** failures identify the semantic assertion rather than an accidental register-clobbering artifact

#### Scenario: FD helper second failure unwinds
- **WHEN** the helper's first descriptor check returns `-EBADF` but its second check intentionally returns success
- **THEN** the helper reaches the named failure-path fixture marker, restores its return stack, and the surrounding test continues

### Requirement: Scheduler probes report tri-state evidence

The scheduler probe SHALL report the accelerator, attempt number, readiness/completion state, selected transcript, and final classification. Contradictory or malformed product evidence MUST be `FAIL`; unavailable or unsupported execution environments MUST be `BLOCKED`; only satisfied evidence requirements may be `PASS`.

#### Scenario: Scheduler evidence is malformed
- **WHEN** a probe reaches the target process but emits malformed, contradictory, or insufficient scheduler records
- **THEN** the result is `FAIL` with the exact validation errors and transcript context

#### Scenario: Scheduler probe is unavailable
- **WHEN** no configured accelerator/attempt can establish the required probe readiness
- **THEN** the result is `BLOCKED`, not `FAIL` or `PASS`, and includes all attempted accelerators and reasons

#### Scenario: Scheduler probe passes
- **WHEN** the required target, exact intervals, ordered frame events, round-trip threshold, and no-exception evidence are present
- **THEN** the result is `PASS` and prints the accelerator, attempt, transcript identity, and evidence summary

#### Scenario: Probe attempts include a fatal marker-rich failure
- **WHEN** one attempt emits a required marker but also emits an exception or panic, while another attempt satisfies the completion predicate without fatal output
- **THEN** the marker-rich attempt is classified `FAIL` and selection retains the semantic `PASS` before comparing readiness, completion, or evidence richness

#### Scenario: Selected Rust probe result is not acceptable
- **WHEN** `test_vibit_rust()` receives a selected `FAIL`/`BLOCKED` result or a transcript containing a shared fatal marker
- **THEN** it rejects the result before Rust ELF entry, provenance, interval, or scheduler checks run

### Requirement: Completion changelog is traceable

The aggregate completion entry SHALL accurately identify the date, reviewed scope, modified files, validation results, limitations, and delivery status. It MUST NOT claim source changes, tests, or completed delivery actions that did not occur.

#### Scenario: Planning-only work is recorded
- **WHEN** this proposal is created without implementation or validation execution
- **THEN** the changelog task remains pending and no implementation-complete claim is added by this proposal

#### Scenario: Implementation completion is recorded
- **WHEN** implementation and validation finish
- **THEN** the aggregate entry links each claim to the actual files and commands/results, records `BLOCKED` limitations distinctly, and states whether commit, push, draft PR, final review, and opening occurred
