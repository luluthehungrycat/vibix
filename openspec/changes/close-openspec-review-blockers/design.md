## Status

Implementation, acceptance validation, and adversarial review are complete.
Runtime evidence is traceable to bounded command logs: Rust ELF and the
257-page lifecycle select TCG `PASS` after KVM readiness-without-completion
`FAIL` attempts; no runtime probe is currently `BLOCKED`. Delivery remains
pending for the orchestrator workflow.

## Context

This change closes review blockers spanning the ELF loader/`exec()` path, DEBUG scheduler evidence, the Python/QEMU harness, the root build path and CI, ABI fixtures, and changelog traceability. The current tree already contains related implementations and probes, but the review evidence is not strong enough to establish transactional failure behavior, exact disjoint executable ranges, deterministic classification, or reproducible toolchain ownership.

The design must preserve VIBIX’s bare-metal constraints: no copied foreign kernel or GPL source, no sibling userspace source in this repository, and no production dependency on generated sibling sources. VIBIT, vish, and GVIBU remain external source owners; this repository may stage only their binaries for integration tests.

## Goals / Non-Goals

**Goals:**

- Define rollback ownership for every page-table, physical-page, and loader/provenance allocation made during a failed ELF `exec()` attempt.
- Ensure fork roots eagerly own distinct copies of all present non-huge user 4 KiB leaf frames; user huge mappings are rejected rather than shared, and COW is out of scope.
- Build flat images under fresh transaction-owned roots with checked size/range arithmetic and a post-code stack.
- Represent executable PT_LOAD coverage as exact, validated disjoint intervals rather than a bounding range that can admit a gap.
- Make the large-ELF lifecycle and Rust scheduler probes prove the required evidence and produce reproducible `PASS`, `FAIL`, or `BLOCKED` outcomes.
- Ensure root Make builds and CI select the same stable Rust toolchain and target.
- Make every FD fixture call preserve live values across syscalls according to the documented full-register-clobber ABI.
- Establish acceptance evidence and a review/fix iteration gate before delivery.

**Non-Goals:**

- No changes to VIBIT, vish, or GVIBU source repositories.
- No redesign of the syscall numbering, scheduler policy, ELF format, or page-table architecture beyond the failure-atomicity, eager fork isolation, and evidence contracts required here.
- No copy-on-write implementation and no claim of universal successful old-root reclamation; safe old-root reclamation is a follow-up ownership/reference-counting change.
- No weakening of DEBUG-off behavior, anti-cheat rules, or bounded QEMU execution.
- The planning session itself made no implementation edits; subsequent apply sessions are governed by the task checklist and validation gates below.

## Decisions

### 1. Use an explicit per-exec transaction journal

The ELF load and stack setup will record pages, page-table metadata, and other owned allocations created for the attempted image. On any failure, a single rollback path will unmap or restore the attempted mappings, return owned physical pages and metadata to the allocator, invalidate affected TLB entries, and leave the process’s prior image usable. Commit occurs only after ELF mappings, stack pages, process state, and syscall return state are all ready.

Prefer a journal/guard over ad-hoc cleanup branches because failures can occur in both `elf::load` and subsequent stack allocation, and the rollback contract must cover partial progress. The implementation must use existing VIBIX paging/PMM interfaces or extend them with ownership-aware helpers; it must not copy foreign kernel implementations. A failed rollback itself must be reported as a hard test failure rather than silently continuing.

### 1a. Eagerly clone fork user leaves

When `copy_user=true`, the new root gets a private PT and a newly allocated
physical 4 KiB frame for every present user PTE at or above `USER_CODE_ADDR`.
The source flags are preserved, but the child PTE is published only after the
frame copy succeeds. User 1 GiB/2 MiB mappings are rejected because they have
no safe eager 4 KiB clone path. Partial construction frees only child-owned
frames and newly allocated table pages; parent mappings and frames are never
modified or freed. No COW behavior is introduced.

The copy reads the parent's existing user virtual address and writes through a
reserved kernel-only scratch page. The child frame is temporarily mapped at
that scratch address while interrupts are disabled for the complete
map/copy/unmap sequence. Scratch page-table branches are provisioned
transactionally before cloning; unavailable branches reject fork without
leaking newly allocated table pages. The scratch page is excluded from ELF,
flat, and user mappings. A successful or failed fork removes only the scratch
leaf and empty PD/PT branches created by that transaction; pre-existing active
root branches remain borrowed. The child PT is published before copying its
leaves so rollback can find and free every child-owned eager frame.

### 1b. Fresh-root flat execution

Flat images use a fallible builder that creates a fresh PML4, checks size and
page arithmetic against the supported user range, maps/copies code pages, and
places one stack page after the code. The root, entry, and RSP are published
only after all steps succeed. On failure only the new root is destroyed; the
old CR3, process fields, and syscall return state remain untouched. Reclaiming
an old root after successful replacement is explicitly deferred.
Every newly allocated flat code page is zeroed before the optional source copy.
When a page has no remaining source bytes, the loader skips forming a source
pointer and performs no copy; small binaries therefore cannot expose stale PMM
bytes through zero-filled tails or rely on out-of-object pointer arithmetic.

### 1c. Observable stack rollback proof

The DEBUG rollback fixture invokes the shared image builder with failure after
exactly one stack page. It checks the reported mapped-page count, unchanged
active root and inherited mapping, absence of publication, and deterministic
allocator reuse after the builder has destroyed its private root. It never
walks a destroyed root; the final marker is emitted only after every rollback
case completes.

### 2. Preserve interval sets end-to-end

Scheduler target registration will parse every executable PT_LOAD into an ordered, non-overlapping half-open interval `[start, end)`, retaining disjoint intervals separately and merging only true overlap or adjacency when the evidence contract explicitly permits it. DEBUG records will serialize the complete interval set. The Python validator will require the entry and every observed executable RIP to belong to one exact interval, reject malformed/overlapping interval records, and retain compatibility only with the new explicit record format.

Using a min/max envelope is rejected because it turns unmapped gaps between executable segments into apparently valid evidence. A single broad range is not sufficient for the large-ELF or Rust scheduler acceptance gates.

### 3. Make probes evidence-driven and tri-state

The bounded QEMU runner will retain accelerator name, attempt number, readiness state, completion state, and the selected serial transcript. Missing prompt/readiness/completion evidence will use deterministic markers and bounded retry rules. The scheduler probe result will be explicitly `PASS`, `FAIL`, or `BLOCKED`: assertion contradictions and malformed evidence are `FAIL`; unavailable accelerators, unsupported execution environments, or a bounded run that cannot establish readiness are `BLOCKED` with a reason. A blocked probe cannot be silently counted as passed, and the final report will display the accelerator, attempt, and transcript selected for diagnosis.

Attempt selection ranks semantic classification first (`PASS` over `FAIL` over
`BLOCKED`), then completion, readiness, and evidence richness. Exception,
panic, and equivalent fatal transcripts are always `FAIL`, even when a marker
was also observed, so a valid PASS attempt is retained over a marker-rich
failure.

The ordinary VIBIT boot and interactive handoff probes use the same structured
attempt record as the scheduler probes. Their reports include accelerator,
attempt, readiness, completion, selected transcript, classification, and a
deterministic reason; missing serial readiness is `BLOCKED`, while product
markers missing after readiness are `FAIL`.

The synthetic large-ELF lifecycle gate will require its scheduler evidence in addition to the first marker, reaper/respawn marker, second marker, and no-exception checks. This prevents lifecycle success from masking an unproven scheduling path.

### 4. Make toolchain selection explicit at both entry points

The root Makefile will invoke Cargo through an explicit stable toolchain (or an equivalent repository-controlled stable selector) and retain the kernel target and code-model flags. CI will install/select that same stable toolchain and target before invoking both direct Cargo checks and root Make targets. The workflow will continue to validate manifest boundaries and run the canonical runtime gates rather than replacing them with host-only Rust checks.

### 5. Treat syscall clobbering as a fixture invariant

FD regression assembly will save any live argument, descriptor, buffer, loop, or comparison value across each syscall before reusing it. `SYSCALL.md` remains the source of truth: `rcx` and `r11` are handled according to the entry/return mechanism and all other registers are treated as clobbered. Documentation changes are limited to clarifying the already-intended VIBIX ABI if fixture behavior exposes ambiguity; the kernel ABI is not broadened for test convenience.

### 6. Separate implementation completion from delivery completion

Tasks will first implement and validate each blocker, then require an adversarial review/fix loop. Only after repeated review reports no merge blockers may the work be committed and pushed on a feature branch, opened as a draft PR to `main`, reviewed one final time, and changed from draft to open. Any new blocker sends the work back to the fix/review gate.

## Risks / Trade-offs

- **[Risk] Rollback cleanup may miss shared page-table structures or stale TLB entries.** → Track only allocations/mappings owned by the attempted transaction, test failures at each allocation boundary, and invalidate/restore before resuming the old image.
- **[Risk] More exact scheduler records increase serial volume and perturb timing.** → Bound emission after the required evidence, use compact interval serialization, and preserve accelerator retries and transcript capture.
- **[Risk] Environment-specific QEMU behavior can be mistaken for a product failure.** → Use explicit tri-state classification with reason codes; never convert `BLOCKED` into `PASS`.
- **[Risk] Pinning stable Rust changes the locally selected Cargo behavior.** → Use the same explicit selector in Make and CI, report versions, and retain the canonical root build/test gates.
- **[Risk] Assembly fixture saves obscure the original regression.** → Add focused markers/assertions around the affected FD cases and keep the ABI statement adjacent to the fixture contract.
- **[Risk] Changelog repair could erase useful historical evidence.** → Preserve factual results and limitations while correcting dates, file lists, and status wording; do not claim validation that was not run.

## Migration Plan

1. Implement the transaction, interval, harness, toolchain, fixture, and traceability changes in separate reviewable commits or task groups.
2. Run targeted tests after each group, then the canonical build, anti-cheat, and bounded QEMU suites where the environment permits.
3. If rollback or evidence validation regresses, revert the affected implementation group; no userspace sibling source migration is required.
4. Complete the adversarial review/fix iteration gate before branch delivery and PR operations.

## Open Questions

- Which existing PMM/page-table helper is the least invasive place to expose ownership-aware unmap/free operations without changing unrelated paging behavior?
- Which exact scheduler interval record encoding best balances parseability and serial volume while preserving every disjoint interval?
- Which probe conditions are environment-blocked versus product-failing, and can the existing accelerator discovery distinguish them reliably on CI hosts?
