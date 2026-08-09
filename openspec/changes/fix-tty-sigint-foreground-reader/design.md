## Context

The singleton TTY records `waiting_pid` when a process blocks in `read()`. Scheduler-side input polling later calls `handle_ctrlc()`, where SIGINT is currently assigned to `current_pid()`. During polling this can be idle PID 2 rather than the blocked reader.

## Goals / Non-Goals

**Goals:** route Ctrl-C to the blocked TTY reader, preserve canonical line discard and normal wakeup, and handle stale ownership safely.

**Non-Goals:** full process-group/job-control support, signal ABI changes, or sibling-repository edits.

## Decisions

- Use the live process recorded in `waiting_pid`; never fall back to `current_pid()`.
- Keep line clearing, pending-signal assignment, and newline publication in the TTY handler; let `push_byte()` perform the existing Ready transition and scheduling request.
- Clear stale ownership without mutating an unrelated process.

## Risks / Trade-offs

- [Stale reader] → validate process state and clear ownership.
- [Wakeup duplication] → retain the single `push_byte()` wakeup path.
- [Deferred signal observation] → test both pending state and existing return-path behavior.

## Migration Plan

No migration. Apply VIBIX TTY/test changes, run focused checks plus existing kernel/VIBIT suites. Rollback is reverting those VIBIX files.

## Open Questions

A future job-control change may replace the single-reader field with a foreground process group.
