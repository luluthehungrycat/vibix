## Status

Implementation, acceptance validation, and adversarial review are complete. The
bounded Rust ELF probes selected TCG `PASS` after KVM attempts reached
readiness without completion; canonical, focused, anti-cheat, Python, and
strict OpenSpec checks also passed. Delivery remains a separate pending gate.

## Why

The recent ELF, scheduler-evidence, VIBIT/vish, FD, and CI work has useful runtime evidence, but an aggregate review identified correctness and traceability gaps that prevent treating the work as merge-ready. The gaps include non-transactional `exec()` failure handling, lossy executable-range evidence, nondeterministic harness classification, an unpinned root build path, ABI-incorrect fixtures, and a changelog entry whose claims are not sufficiently traceable to the reviewed work.

## What Changes

- Make ELF `exec()` setup transactional: a stack-page allocation or mapping failure must roll back all pages and allocator/provenance state created for the attempted image, without leaving a partially replaced process address space.
- Make fork address spaces isolated without COW: eagerly clone every present non-huge user 4 KiB leaf, reject user huge mappings safely, and reclaim only child-owned frames on partial construction failure.
- Build flat binaries through the same fresh-root transaction boundary as ELF, with checked arithmetic, a supported user-range cap, and the stack placed after code; successful old-root reclamation remains a separate follow-up.
- Zero every newly allocated flat code page before copying source bytes, including no-copy pages, and prove small-binary tails do not expose seeded stale PMM data.
- Prove stack rollback through the shared image builder with an injected failure after exactly one stack page, unchanged active-root state, no publication, and deterministic allocator reuse.
- Preserve the exact disjoint executable `PT_LOAD` intervals in DEBUG scheduler evidence and validate those intervals rather than collapsing them into one min/max span.
- Require correlated scheduler evidence, not only entry/no-exception evidence, in the synthetic large-ELF lifecycle test.
- Pin the root `Makefile` Rust invocation to stable Rust and make the CI workflow assert the same toolchain/target boundary.
- Make missing VIBIT/vish harness prompts and readiness/completion markers deterministic failures instead of timing-dependent or ambiguous outcomes.
- Update FD regression fixtures and, where necessary, `SYSCALL.md` so every syscall call observes the documented full-register-clobber ABI.
- Exercise the FD helper's second-failure cleanup path and verify it reaches a named fixture marker without corrupting the return stack.
- Report the scheduler probe accelerator, attempt, and selected transcript, and classify an unavailable/unsupported probe as `BLOCKED` distinctly from a failed assertion (`FAIL`).
- Make `test_vibit_rust()` reject any selected `FAIL`/`BLOCKED` or fatal transcript before running Rust ELF evidence checks.
- Correct the aggregate completion entry in `CHANGELOG.md` so dates, files, validation claims, and known limitations are traceable and do not imply work that was not performed.
- Add an explicit implementation review/fix iteration gate and delivery tasks for repeated adversarial review, branch commit/push, draft PR creation, final PR review, and opening the PR only after a clean review.

## Capabilities

### New Capabilities

- `review-blocker-closure`: Transaction-safe ELF execution, lossless scheduler interval evidence, deterministic VIBIT/vish and scheduler probe classification, stable Rust validation, ABI-correct FD fixtures, and auditable completion traceability.

### Modified Capabilities

<!-- No existing repository-local main capability specs exist; this change defines one aggregate VIBIX review/validation capability. -->

## Impact

- Kernel implementation: `kernel_rust/src/process.rs`, `kernel_rust/src/elf.rs`, and `kernel_rust/src/paging.rs` for rollback-safe ELF/flat `exec()` setup, eager fork isolation, and ownership-aware cleanup; `kernel_rust/src/scheduler_evidence.rs` for exact executable intervals and evidence records.
- Harness and validation: `test_kernel.py`, `Makefile`, and `.github/workflows/rust.yml` for lifecycle assertions, deterministic classifications, scheduler probe reporting, and stable-toolchain validation.
- ABI fixtures/documentation: `userspace/vibix_user_test.inc` and `SYSCALL.md` as needed to enforce the stated syscall clobber contract.
- Traceability: `CHANGELOG.md`.
- No VIBIT, vish, or GVIBU source changes are in scope; sibling userspace source remains in sibling repositories and only their binaries may be staged here.
- Implementation and acceptance validation for this change are recorded in the
  aggregate changelog, `NEXT_SESSION.md`, and task checklist. The
  implementation preserves VIBIX’s no-foreign-kernel-source/no-GPL-code
  constraints and uses the canonical build, anti-cheat, test, and bounded QEMU
  workflows. The repeated adversarial reviews and final clear source-level
  verdict are complete; delivery tasks remain separate pending gates.
