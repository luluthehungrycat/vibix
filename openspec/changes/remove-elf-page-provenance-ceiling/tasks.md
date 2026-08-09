## 1. Scalable provenance

- [x] 1.1 Replace the fixed 256-entry tracker with a reclaimed kernel-owned metadata arena supporting 4096 tracked pages per load without general-heap ownership.
- [x] 1.2 Preserve same-load overlap reuse and record each newly allocated page before later PT_LOAD processing can reuse it.
- [x] 1.3 Add distinct metadata-resource failure handling without changing malformed-input or PMM exhaustion semantics.

## 2. Transactional failure cleanup

- [x] 2.1 Define and implement target-PML4 unmap/frame-release cleanup for every page tracked by a failed load.
- [x] 2.2 Free provenance metadata on success and every error path while preserving inherited mappings; cleanup resets fields directly to avoid recursive `Drop` assignment.
- [x] 2.3 Verify TLB/interrupt discipline during rollback and successful ownership transfer.

## 3. Regression coverage

- [x] 3.1 Add a test-time synthetic 257-page ELF fixture and lower-level arena-capacity/cleanup regression; full shell execution remains deferred because VIBIT handoff is not a reliable fixture entry point.
- [ ] 3.2 Add failure-after-partial-map coverage for truncation, PMM exhaustion, and metadata exhaustion (deferred: no existing failure-injection mechanism is available).
- [x] 3.3 Re-run Rust ELF, kernel, VIBIT, synthetic provenance, and anti-plagiarism validation.
