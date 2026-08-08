## Bounded remediation status

- [x] Read Oracle Phase 1 findings and confirm the Candidate/Active/Retired rewrite is absent.
- [x] Correct DEBUG return-frame offsets and labels in both assembly return paths.
- [x] Inspect `elf.rs` overlap handling and confirm `translate_in_pml4` is a general helper not used by the loader.
- [x] Reconcile documentation with the current harness: `test_kernel.py` has 21 required default markers and 7 VIBIT markers.
- [x] Make `test_vibit_rust` require entry execution plus three DEBUG global IRQ observations; do not label global IRQ lines as Rust-process round trips.
- [x] Add DEBUG-gated ELF header, rounded PT_LOAD ranges, map/zero/copy, target/active-root translation, verified physical-byte, exec, and raw page-fault capture.
- [x] Correct the test target's post-replacement archive creation to use ustar, preserving VIBIT init resolution.
- [x] Prove the current minimal overlap-loader defect diagnostically — PT_LOAD[0] writes frame `0x29c000`, then PT_LOAD[1] remaps the same rounded page to fresh frame `0x29d000`, leaving entry bytes zero.
- [x] Validate that the fix reuses the original entry-page frame and preserves both entry code and second-segment `OK` bytes.
- [x] Prove current Rust ELF entry output and preserved shared-page provenance in the focused VIBIT Rust probe; global IRQ lines remain only global observations.
- [ ] Prove Rust-process scheduling round trips — **BLOCKED**; serial IRQ lines lack PID/instruction-range correlation.
- [x] Apply the Oracle-approved load-local `LoadedPage` provenance fix; preserve partial-page copy ordering and return `ElfError::Oom` on metadata exhaustion.
- [ ] Confirm historical GPF root cause or fix — **OUT OF SCOPE/BLOCKED**; the rewrite where it was reported is absent.

## Validation record

- [x] `make clean && make`
- [x] `make clean && make DEBUG=1`
- [x] `make test` — 21/21 markers
- [x] `make test_vibit` — 7/7 markers
- [x] `make test_vibit_rust` — Rust `OK`, no exception, preserved entry-page provenance
- [ ] PID/range-correlated Rust-process scheduling evidence — separately scoped optional work
