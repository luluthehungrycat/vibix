## Why

Timer-side TTY polling currently processes Ctrl-C while the idle process is current. Because Ctrl-C is assigned through `current_pid()`, SIGINT can be delivered to idle PID 2 instead of the blocked foreground reader. This leaves the shell awakened but not interrupted.

## What Changes

- Track the TTY reader/foreground ownership needed for signal routing.
- Route Ctrl-C to the blocked foreground reader while preserving wakeup and line-discard behavior.
- Define behavior when no reader is blocked and when the recorded reader exits or changes state.
- Add bounded kernel/VIBIT coverage for Ctrl-C ownership and wakeup.

## Capabilities

### New Capabilities
- `tty-foreground-sigint`: Routes terminal-generated SIGINT to the foreground or blocked TTY reader and wakes that reader safely.

### Modified Capabilities

<!-- No existing capability specifications are present in the repository-local OpenSpec tree. -->

## Impact

- Affects VIBIX TTY line discipline, process signal state, and scheduler-driven input polling.
- Tests remain VIBIX-owned; no VIBIT, vish, or other sibling source changes are in scope.
