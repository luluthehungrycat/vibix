## 1. Baseline and transaction design

- [ ] 1.1 Validation owner (orchestrator) records the current review evidence, affected files, existing OpenSpec status, and a clean-worktree/baseline test result before implementation.
- [x] 1.2 Trace every allocation and mapping performed by ELF `exec()` and stack setup in `kernel_rust/src/process.rs`, `kernel_rust/src/paging.rs`, and the loader path; identify the exact ownership boundary for rollback.
- [x] 1.3 Define deterministic failure-injection or fixture coverage for loader failure, page-table failure, and stack allocation failure without importing foreign kernel source or changing sibling repositories.

## 2. Transactional ELF exec

- [x] 2.1 Implement an explicit per-exec transaction journal/guard that records new mappings, physical pages, page-table metadata, provenance state, and TLB effects owned by the attempted image.
- [x] 2.2 Route every ELF-load and stack-setup failure through one rollback path that restores the prior image or defined process state, reclaims owned resources, and invalidates affected translations.
- [x] 2.3 Commit new process/syscall state only after ELF mappings and all stack pages succeed; preserve the documented VIBIX errno behavior on failure.
- [x] 2.4 Add focused rollback assertions for truncated-later-segment, PMM-exhaustion, metadata-exhaustion, and stack-allocation failure cases, including allocator/resource reclamation and absence of partial mappings.
- [x] 2.5 Build flat images under a fresh fallible root with checked arithmetic, a supported user-range cap, and a post-code stack; publish root/entry/RSP only after success and document old-root reclamation as deferred.
- [x] 2.6 Add focused flat allocation-failure, oversize-rejection, and old-root no-overwrite coverage.
- [x] 2.6a Add focused small-binary coverage proving zero-length source tails do not form out-of-object pointers, zero newly allocated code pages before copying, and do not expose seeded stale PMM bytes.

## 2a. Fork isolation

- [x] 2a.1 Change `create_pml4(copy_user=true)` to eagerly clone every present non-huge user 4 KiB leaf into a distinct child frame, preserving flags and rejecting user huge mappings without COW.
- [x] 2a.2 Publish child PTEs only after copy success; reclaim only child-owned leaves/table frames on partial failure and update fork failure cleanup/ownership metadata.
- [x] 2a.3 Add focused parent/child frame distinction, child write isolation, and injected post-first-leaf PMM-failure coverage.
- [x] 2a.4 Copy fork leaves through the reserved kernel-only scratch mapping with transactional branch provisioning and user/ELF/flat range exclusion.
- [x] 2a.5 Make scratch ownership failure-atomic, publish child PTs before leaf copies for rollback discovery, and prove success/failure scratch cleanup with PMM reuse evidence.
- [x] 2.7 Replace the weak pre-destroy stack check with shared-builder evidence: exactly one mapped stack page, unchanged active root/state, no publication, deterministic reuse, and final-marker ordering.

## 3. Exact scheduler executable intervals

- [x] 3.1 Replace the scheduler evidence min/max executable envelope with an exact ordered set of disjoint half-open executable PT_LOAD intervals, merging only explicitly permitted overlap/adjacency.
- [x] 3.2 Emit the complete interval set in DEBUG target records and update scheduler event validation to require interval membership for entry and every observed RIP.
- [x] 3.3 Add a disjoint-segment fixture/assertion proving a RIP in an unmapped gap is rejected and valid RIPs in either segment are accepted.

## 4. Deterministic lifecycle and scheduler probes

- [x] 4.1 Require correlated scheduler evidence in the synthetic large-ELF lifecycle test in addition to ordered markers, reaper/respawn continuation, and no-exception checks.
- [x] 4.2 Refactor VIBIT/vish readiness, prompt, and completion handling so missing required markers produce named, bounded, deterministic outcomes rather than timeout-dependent success.
- [x] 4.3 Record accelerator, attempt number, readiness/completion state, selected transcript, and reason for every bounded scheduler/lifecycle probe attempt.
- [x] 4.4 Implement and test tri-state probe classification: `PASS` only for complete evidence, `FAIL` for malformed/contradictory product evidence, and `BLOCKED` for unavailable/unsupported execution environments; Rust result checks also reject selected non-PASS or shared fatal-marker transcripts.
- [x] 4.5 Ensure the final harness report distinguishes `BLOCKED` from `FAIL` and never silently promotes either to `PASS`.
- [x] 4.6 Rank probe semantic classification before completion/readiness/evidence and reject exception/panic transcripts as `PASS`, with focused selection coverage.
- [x] 4.7 Route ordinary VIBIT boot and shell handoff diagnostics through structured accelerator/attempt/readiness/completion/transcript/reason records.

## 5. Stable Rust root and CI validation

- [x] 5.1 Pin the root `Makefile` Cargo invocation to stable Rust while preserving `x86_64-unknown-none`, release/debug feature selection, and `RUSTFLAGS` code-model behavior.
- [x] 5.2 Align `.github/workflows/rust.yml` installation, direct Cargo checks, and root Make invocations with the same explicit stable toolchain and target.
- [x] 5.3 Keep manifest-boundary checks and canonical root build/test gates in CI; document any unsupported host-only test command rather than weakening runtime validation.

## 6. FD fixture ABI compliance

- [x] 6.1 Audit every syscall in `userspace/vibix_user_test.inc` for values live after return and save/reload them according to the full-register-clobber contract.
- [x] 6.2 Correct the FD regression fixture so descriptor, pointer, length, loop, and comparison values are not accidentally taken from clobbered registers.
- [x] 6.3 Update `SYSCALL.md` only where needed to make the tested clobber/return contract unambiguous, keeping VIBIX syscall numbers and semantics unchanged.
- [x] 6.4 Run the focused FD regression and confirm failures identify FD semantics rather than fixture ABI misuse.
- [x] 6.5 Repair and exercise the helper's second-failure cleanup path, including a named failure-path marker and continued return to the surrounding fixture.

## 7. Changelog traceability

- [x] 7.1 Reconcile the aggregate completion entry in `CHANGELOG.md` against the actual implementation diff, OpenSpec artifacts, commands, results, limitations, and delivery state.
- [x] 7.2 Preserve evidence-backed results and explicitly label anything `BLOCKED`, not run, deferred, or still awaiting PR delivery; do not claim delivery completion before the orchestrator workflow.

## 8. Canonical validation and acceptance gate

- [x] 8.1 Run `openspec status --change close-openspec-review-blockers` and strict OpenSpec validation after artifacts and implementation are complete.
- [x] 8.2 Run the canonical VIBIX validation where feasible: `make clean && make`, `make test`, `make test_vibit`, focused rollback/FD/lifecycle/scheduler targets, `make clean && make DEBUG=1`, anti-cheat, Python syntax checks, and bounded QEMU probes.
- [x] 8.3 Verify acceptance criteria: transactional rollback has no leaked/partial state; eager fork leaves are isolated without COW; flat exec is fresh-root and bounded; exact intervals reject gaps; large-ELF requires scheduler evidence; stable Rust is used by Make and CI; missing prompts are deterministic; FD fixtures obey the ABI and failure cleanup; probe reports include accelerator/attempt/transcript and tri-state status; changelog claims are traceable. Evidence: `make test_elf_rollback` passed every rollback/fork/scratch/flat marker; `make test_vibit_rust` and `make test_vibit_rust_large` selected TCG `PASS` after bounded KVM `FAIL` attempts and reported PID/CR3/RIP-correlated scheduler evidence; the default suite passed all required FD markers; strict OpenSpec, anti-cheat, Python syntax, and diff checks passed.
- [x] 8.4 Validation owner (fixer) records the exact bounded commands, command results, accelerator attempts, selected transcripts, deterministic classifications, environment prerequisite, warnings, and remaining delivery gates in `CHANGELOG.md` and `NEXT_SESSION.md`; no `BLOCKED` runtime condition remained after TCG fallback. Delivery tasks remain pending for the orchestrator.

## 9. Adversarial review/fix iteration gate

- [x] 9.1 Request an adversarial review of the complete implementation and validation evidence, including rollback safety, interval soundness, harness classification, CI reproducibility, ABI correctness, and traceability. Evidence: repeated adversarial reviews on 2026-08-22 covered the full scope; the latest review found no source-level blockers.
- [x] 9.2 Fix every merge blocker found by review, rerun the affected targeted checks and canonical validation, and repeat adversarial review until no merge blockers remain. Evidence: the reviewed fixes were followed by the recorded rollback, Rust ELF, large-lifecycle, strict OpenSpec, anti-cheat, Python, and diff validations; the final 2026-08-22 review verdict was clear.
- [x] 9.3 Do not proceed to delivery while any unresolved finding can invalidate an acceptance criterion or is incorrectly classified as `BLOCKED`. Evidence: the final 2026-08-22 source-level verdict found no unresolved blocker; delivery tasks 10.1–10.4 remain intentionally pending for the orchestrator workflow.

## 10. Delivery and final verification

- [ ] 10.1 Commit only the intended implementation, OpenSpec, and traceability files on a feature branch after the review/fix gate is clean; inspect status, diff, and recent history first.
- [ ] 10.2 Push the feature branch and open a draft PR against `main`, preserving the validation summary and any explicitly documented environment blocks.
- [ ] 10.3 Perform a final PR review over all included commits and the base diff; fix and revalidate any newly found blocker.
- [ ] 10.4 Change the draft PR to open only after the final PR review is clean and all acceptance criteria are satisfied or transparently marked `BLOCKED` by the orchestrator.
