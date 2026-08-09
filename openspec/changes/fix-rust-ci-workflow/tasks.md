## 1. Confirm CI contract and runner prerequisites

- [x] 1.1 Verify `.github/workflows/rust.yml` is the only workflow in scope and record the existing Cargo build/test, root Makefile, and `make test` expectations.
- [x] 1.2 Determine the stable-toolchain command and runner packages required for `x86_64-unknown-none`, NASM, linker/objcopy, Python, and QEMU on `ubuntu-latest`.
- [x] 1.3 Validate the target-aware Rust test strategy for the `#![no_std]` staticlib (`cargo test` versus a target-aware check/no-run form) without changing kernel source or Cargo metadata.

## 2. Implement the workflow boundary

- [x] 2.1 Add explicit stable Rust toolchain setup and install/verify the `x86_64-unknown-none` target before any kernel Cargo command.
- [x] 2.2 Replace root-relative Cargo invocations with `kernel_rust` working-directory or explicit `--manifest-path kernel_rust/Cargo.toml` commands, including an explicit kernel target.
- [x] 2.3 Keep Rust build and test/check steps separately named and make the manifest/working-directory boundary visible; do not add a root Cargo workspace or manifest.
- [x] 2.4 Add or retain runner dependency setup needed for the existing VIBIX Makefile build and test commands.
- [x] 2.5 Preserve separate repository-root build and `make test` steps, with no weakening, omission, or accidental execution from `kernel_rust`.

## 3. Validate the complete CI contract

- [x] 3.1 Inspect the final workflow to confirm no Cargo command relies on the repository root and `kernel_rust/Cargo.toml` remains the referenced manifest.
- [x] 3.2 Run the equivalent kernel Rust build/test commands using the configured no-std target and confirm they pass against `kernel_rust`.
- [x] 3.3 Run the repository build and `make test` checks from the repository root and confirm existing kernel/anti-cheat/boot validation remains active.
- [x] 3.4 Validate the workflow YAML and review CI logs for clear, independently attributable Rust versus repository build/test failures.
