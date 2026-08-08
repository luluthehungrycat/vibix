## ADDED Requirements

### Requirement: Ctrl-C targets the blocked TTY reader
The TTY SHALL deliver terminal-generated SIGINT to the live process recorded as the blocked TTY reader, not to the scheduler's current process.

#### Scenario: Idle polling wakes the shell reader
- **WHEN** a shell is recorded in `waiting_pid`, idle PID 2 is current, and Ctrl-C arrives
- **THEN** the shell's SIGINT pending bit is set and PID 2 is unchanged

#### Scenario: No reader is waiting
- **WHEN** Ctrl-C arrives while `waiting_pid` is zero
- **THEN** the partial line is discarded and no unrelated process receives SIGINT

### Requirement: Ctrl-C wakes the owning reader through normal TTY flow
After Ctrl-C, the TTY SHALL make the blocked reader runnable through the same completed-input path used for normal input.

#### Scenario: Blocked reader resumes
- **WHEN** Ctrl-C clears the line and the reader is `Blocked`
- **THEN** a newline is available, the reader becomes `Ready`, ownership is cleared, and scheduling is requested

#### Scenario: Reader ownership is stale
- **WHEN** `waiting_pid` does not identify a live blocked process
- **THEN** stale ownership is cleared without mutating an unrelated process or panicking

### Requirement: Ctrl-C preserves canonical line behavior
The TTY SHALL clear the partial canonical line and emit the configured `^C` echo before publishing the wakeup newline.

#### Scenario: Partial input is cancelled
- **WHEN** printable characters precede Ctrl-C in canonical mode
- **THEN** those characters are absent from completed input and echo is emitted when enabled
