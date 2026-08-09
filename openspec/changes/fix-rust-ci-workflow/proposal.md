## Why

PR #9 exposed that `.github/workflows/rust.yml` invokes Cargo from the repository root, where VIBIX intentionally has no `Cargo.toml`. The workflow therefore fails before validating the actual kernel Rust crate; CI must target `kernel_rust` while retaining the repository-level kernel checks.

## What Changes

- Update the VIBIX-owned Rust workflow so Cargo operates on `kernel_rust/Cargo.toml` (via working directory or explicit manifest path), never an assumed root manifest.
- Install/configure the Rust target and toolchain required by the `x86_64-unknown-none` no-std kernel crate before Cargo validation.
- Preserve the existing kernel build and test checks by running the repository Makefile validation in CI alongside the corrected Rust crate build/test commands.
- Make CI commands and their working directories explicit enough that future changes cannot silently reintroduce the root-Cargo assumption.

## Capabilities

### New Capabilities

- `rust-kernel-ci`: CI validation for the VIBIX kernel Rust crate and repository-level build/test checks, including explicit no-std target/toolchain setup.

### Modified Capabilities

- None.

## Impact

- Affected workflow: `.github/workflows/rust.yml`.
- Affected CI dependencies: Rust toolchain/target installation and Cargo invocation context.
- Validation scope: `kernel_rust` Cargo commands plus existing VIBIX `make`/test checks.
- No kernel source, userspace source, or sibling repository changes are required.
