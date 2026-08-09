## ADDED Requirements

### Requirement: CI targets the VIBIX kernel Rust manifest
The Rust CI workflow SHALL invoke Cargo against `kernel_rust/Cargo.toml`, either by setting `kernel_rust` as the Cargo step working directory or by passing that manifest explicitly, and SHALL NOT require a repository-root `Cargo.toml`.

#### Scenario: Pull request validates the actual kernel crate
- **WHEN** GitHub Actions runs the Rust workflow from a checkout of VIBIX
- **THEN** the Rust build/test validation resolves `kernel_rust/Cargo.toml` and does not fail because the repository root lacks `Cargo.toml`

#### Scenario: Root manifest is not assumed
- **WHEN** the repository root contains no `Cargo.toml`
- **THEN** the workflow still reaches the kernel Rust Cargo command without creating, requiring, or inferring a root manifest

### Requirement: CI provisions the no-std target and toolchain
The workflow SHALL select a reproducible stable Rust toolchain and SHALL install or verify the `x86_64-unknown-none` target before invoking Cargo for the kernel crate.

#### Scenario: Target setup precedes Cargo
- **WHEN** a fresh GitHub runner starts the workflow
- **THEN** toolchain and `x86_64-unknown-none` target setup completes before kernel Rust build/test validation begins

#### Scenario: Kernel target is explicit
- **WHEN** Cargo builds or checks the kernel crate
- **THEN** the command uses `x86_64-unknown-none` explicitly or through the checked-in kernel configuration, and does not silently fall back to a host target

### Requirement: CI preserves repository kernel checks
The workflow SHALL retain repository-root build and test validation, including the existing Makefile build path and `make test`, after the Rust-specific checks are corrected.

#### Scenario: Kernel image build remains covered
- **WHEN** the workflow reaches repository validation
- **THEN** it runs the VIBIX root build command needed to assemble/link the kernel image and fails if that build fails

#### Scenario: Existing test gate remains covered
- **WHEN** the workflow reaches the test stage
- **THEN** it runs `make test` from the repository root and reports failure rather than skipping or weakening the existing kernel/anti-cheat test gate

### Requirement: CI exposes actionable workflow boundaries
The workflow SHALL keep Rust crate validation and repository Make validation in explicit, independently attributable steps, with working directories and required setup visible in the workflow definition.

#### Scenario: Rust failure is attributable
- **WHEN** a Cargo build/test command fails
- **THEN** the workflow identifies the kernel Rust step and its `kernel_rust` context as the failing boundary

#### Scenario: Repository check failure is attributable
- **WHEN** a root build or `make test` command fails
- **THEN** the workflow identifies the repository-level kernel validation step rather than reporting an ambiguous Cargo-root failure
