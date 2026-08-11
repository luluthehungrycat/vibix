## 1. Define the VIBIX-owned evidence contract

- [x] 1.1 Choose the authoritative DEBUG target-registration record and compact event field format, including PID, CR3/PML4, executable RIP intervals, frame fields, and configured round-trip threshold.
- [x] 1.2 Add a VIBIX-owned fixture/harness setup that keeps the Rust ELF target alive for the bounded threshold without modifying sibling repositories or source.

## 2. Add DEBUG-only kernel correlation points

- [x] 2.1 Implement target registration at the VIBIX ELF handoff with load-derived executable RIP interval metadata and reject conflicting target state per boot.
- [x] 2.2 Emit compact `SCHED_OUT` records from timer- and syscall-driven scheduler paths with target PID, non-target next PID, active CR3, and saved user-frame invariants.
- [x] 2.3 Emit compact `SCHED_IN` records after target CR3 selection and frame restoration with previous PID, expected/active CR3, and restored frame invariants.
- [x] 2.4 Emit `USER_RETURN` only from a subsequent target user-mode interrupt entry, proving the target crossed the return boundary without adding DEBUG-off work.

## 3. Implement correlated harness assertions

- [x] 3.1 Parse one target record and structured scheduler events from fresh serial transcripts, rejecting missing, duplicate, stale, malformed, or conflicting correlation metadata.
- [x] 3.2 Implement the ordered finite-state check for `SCHED_IN → USER_RETURN → SCHED_OUT → SCHED_IN`, excluding same-process selections and validating PID, CR3, RIP-range, CS/SS, RFLAGS, and user-RSP invariants.
- [x] 3.3 Replace the Rust ELF probe's global-IRQ-only scheduling claim with the named configured threshold, exact correlated count, no-exception/no-panic gate, and explicit limitation reporting while preserving existing `OK` and provenance checks.
- [x] 3.4 Add bounded readiness-aware KVM/TCG retry handling and failure output that distinguishes `FAIL` from `BLOCKED` prerequisites without fabricating success.

## 4. Regression validation and handoff evidence

- [x] 4.1 Verify DEBUG-off builds emit no new records and preserve existing default, VIBIT, and focused test behavior.
- [x] 4.2 Run the clean release/debug, anti-cheat, strict OpenSpec, default, VIBIT, Rust ELF, TTY/SIGINT, large-ELF, and rollback validations, recording exact markers/counts and accelerator behavior.
- [x] 4.3 Confirm temporary fixtures/archives and generated binaries are cleaned or restored, inspect scoped `git status`, pass `git diff --check`, and record the final evidence in the project roadmap, next-session notes, and changelog without changing sibling repositories.
