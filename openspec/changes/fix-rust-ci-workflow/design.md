## Context

The current GitHub Actions workflow checks out VIBIX and runs `cargo build` and `cargo test` without a manifest path or working directory. The repository root contains the kernel Makefile and assembly sources, while the only Rust manifest is `kernel_rust/Cargo.toml`; that crate is `#![no_std]`, produces a static library, and defaults to the `x86_64-unknown-none` target through `kernel_rust/.cargo/config.toml`. The workflow must validate both the Rust crate and the repository build/test contract without changing kernel or sibling-repository ownership.

## Goals / Non-Goals

**Goals:**

- Make every Cargo invocation unambiguously target `kernel_rust/Cargo.toml` or execute with `kernel_rust/` as its working directory.
- Install and select a stable Rust toolchain plus the `x86_64-unknown-none` target needed by the no-std kernel crate.
- Retain repository-level `make` and `make test` checks, so CI still validates assembly/linking, image construction, anti-cheat checks, and boot tests.
- Keep the workflow reproducible and fail-fast when the required manifest, target, or build checks are unavailable.

**Non-Goals:**

- No changes to Rust kernel source, Cargo metadata, Makefile behavior, userspace sources, or sibling repositories.
- No conversion of the kernel into a root Cargo workspace.
- No claim that host-target unit tests exist for this no-std staticlib; the crate validation command must remain appropriate to the actual target.

## Decisions

1. **Use an explicit kernel manifest/working-directory Cargo context.**
   The workflow SHALL use `working-directory: kernel_rust` for Rust steps and explicit `--target x86_64-unknown-none` (or an equivalent manifest path plus target) rather than relying on the checkout root. This makes the ownership boundary visible and prevents a future root `Cargo.toml` assumption. An alternative—adding a root workspace—is rejected because it changes repository structure and could make the kernel appear to be a conventional host crate.

2. **Install the no-std target before Cargo validation.**
   Configure a stable toolchain with a minimal profile and add `x86_64-unknown-none` using rustup before Rust commands. The workflow SHALL explicitly select that toolchain/target, while retaining the crate's checked-in `.cargo/config.toml` as the local default. Installing only the host toolchain is rejected because it can silently validate the wrong target.

3. **Separate Rust crate validation from repository validation.**
   Keep dedicated Rust build/test steps scoped to `kernel_rust`, then run the existing root `make` and `make test` checks from the repository root. This preserves the current workflow's Cargo intent and the VIBIX integration checks without making Make depend on Cargo's current directory. Combining everything into a single opaque shell command is rejected because failures would be harder to attribute and a working-directory regression would be easier to miss.

4. **Use CI-readable assertions for the manifest boundary.**
   The workflow SHALL expose the effective working directory/manifest in step configuration and may add a non-mutating assertion that `kernel_rust/Cargo.toml` exists while root `Cargo.toml` is not required. The check must not create a root manifest or alter repository layout.

## Risks / Trade-offs

- [Risk] `cargo test` for a no-std staticlib may not support a normal target test harness. → Mitigation: use the crate's target-aware test/check form agreed by implementation validation, and rely on `make test` for runtime/kernel testing; keep the requirement that the actual kernel manifest is exercised.
- [Risk] `make test` may require NASM, QEMU, Python, and linker utilities not present on a bare runner. → Mitigation: provision/document those runner dependencies in the workflow before invoking Make, and retain the existing test command rather than weakening it.
- [Risk] Rust target availability differs between stable toolchain revisions. → Mitigation: install/select one explicit stable toolchain and fail before build if `rustup target add x86_64-unknown-none` cannot complete.

## Migration Plan

1. Update only `.github/workflows/rust.yml` with toolchain/target setup and explicit Rust/root working directories.
2. Run the workflow on pull requests and pushes, confirming both the kernel Rust validation and the existing Make checks pass.
3. Roll back by reverting the workflow commit; no data or source migration is required.

## Open Questions

- Confirm during implementation whether the stable runner's target-aware `cargo test` can execute for this staticlib, or whether the Rust test step should use `cargo check`/`cargo test --no-run` while `make test` remains the runtime test gate.
