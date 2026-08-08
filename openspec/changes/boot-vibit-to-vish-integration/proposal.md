## Why

VIBIX currently has a VIBIT init binary, but the end-to-end path to the user-facing vish shell is not a reproducible and validated integration procedure. This change makes boot use VIBIT as PID 1, stages the generated NASM vish binary, and verifies a running shell through bounded QEMU checks.

## What Changes

- Add a reproducible VIBIT and NASM vish build-and-staging flow using the documented commands in `../vibit` and `../vish`.
- Stage only generated binaries in VIBIX userspace/initramfs locations; do not copy source code or modify sibling repositories.
- Add explicit, narrow `.gitignore` rules for staged generated binaries without duplicate or overbroad rules.
- Make `INIT=vibit` produce a bootable image with VIBIT as PID 1 and vish available for execution.
- Add bounded integration verification for fork/exec delivery, the shell prompt, input handling, and exit/repeat semantics using the existing harness.
- Preserve the existing NASM vish path and kernel tests.
- Define clear failures when sibling repositories, toolchains, or generated binaries are missing.

## Capabilities

### New Capabilities

- `boot-vibit-to-vish-integration`: Build, stage, and verify the VIBIT-led vish shell integration.

### Modified Capabilities


## Impact

The primary impact is the VIBIX build and staging configuration, userspace initramfs assembly, `.gitignore` policy, and bounded serial QEMU validation in the existing harness. The sibling repositories and their toolchains remain binary providers only: this change does not modify their source code, create their PRs, or commit generated binaries.
