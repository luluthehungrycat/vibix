## 1. TTY ownership and signal routing

- [x] 1.1 Define the foreground-reader ownership invariant around `waiting_pid` and stale process states.
- [x] 1.2 Route Ctrl-C SIGINT to the recorded blocked reader without falling back to `current_pid()`.
- [x] 1.3 Preserve canonical line discard, echo, newline publication, and existing `push_byte()` wakeup behavior.

## 2. Regression coverage

- [x] 2.1 Add a bounded VIBIT/TCP regression proving the blocked foreground reader receives Ctrl-C and the idle scheduler does not.
- [ ] 2.2 Add dedicated no-reader and stale-reader cases; no standalone fixture currently exists, so these edge cases remain open.
- [x] 2.3 Run `make test`, `make test_vibit`, and the focused Ctrl-C validation.
