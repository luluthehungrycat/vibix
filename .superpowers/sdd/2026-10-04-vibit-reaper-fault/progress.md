# SDD ledger — plan: docs/superpowers/plans/2026-10-04-vibit-reaper-fault.md

Pre-flight: shared interfaces: VIBIT formatter output is consumed by the VIBIX serial harness; VIBIT output is built via the onboarding staging script until VISH `nasm` target lands.
Ruling: use existing isolated cloud checkouts on branch `work`, not new worktrees — the onboarding setup workflow requires existing checkouts and forbids creating worktrees — cost if wrong: no per-plan worktree isolation.
Ruling: test reaped PID as a number, not only as the existing prefix — this directly covers the null-pointer fault's missing decimal output — cost if wrong: regex may be stricter than output formatting.

Implementation and verification complete. Fixed the VIBIT `strlen` pointer, made `waitpid` retry blocked parents and remove/release reaped children, preserved queued UART input across debug logging, and strengthened the serial lifecycle checks. `test_kernel.test_vibit()` passed with the NASM shell; `make QEMU=/workspace/.onboarding/bin/qemu-system-x86_64 test_vibit_rust_shell` passed with the Rust shell. Local commits remain pending.
