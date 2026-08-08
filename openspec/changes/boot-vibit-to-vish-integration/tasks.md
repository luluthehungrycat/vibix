## 1. Prerequisites and binary provenance

- [x] 1.1 (VIBIX validation) Confirm `../vibit` and `../vish` are present, clean enough for read-only binary generation, and that NASM, QEMU, and required build toolchains are available; report actionable failures without modifying sibling source.
- [x] 1.2 (VIBIX validation) Record the expected generated outputs and provenance boundary: `../vibit/vibit.bin` from `make -C ../vibit all` and `../vish/vish.bin` from `make -C ../vish nasm`; reject missing, empty, or silently reused outputs.
- [x] 1.3 (Separate sibling-repo follow-up, not VIBIX implementation) If either provider cannot generate a compatible binary, document the dependency and open/track a separate PR in the owning sibling repository; do not edit or commit sibling source from VIBIX.

## 2. VIBIX build and staging

- [x] 2.1 (VIBIX) Update the `INIT=vibit` build flow to invoke the documented VIBIT and vish NASM targets before staging, with explicit command and output errors.
- [x] 2.2 (VIBIX) Copy only generated `vibit.bin` and `vish.bin` outputs into the existing VIBIX staging flow, assemble `/sbin/init` and `/bin/vish` in a temporary initramfs tree, and remove the temporary tree after archive creation.
- [x] 2.3 (VIBIX) Preserve the existing default combined-blob path and NASM vish path; keep the Rust ELF experiment isolated to its existing explicit test path.
- [x] 2.4 (VIBIX) Inspect the resulting `userspace/initramfs.tar` contents and verify `/sbin/init` is VIBIT, `/bin/vish` is the flat NASM binary, and no sibling source files are present.

## 3. Generated-artifact policy

- [x] 3.1 (VIBIX) Review current `.gitignore` coverage and replace or refine overlapping broad rules as needed so `userspace/vibix_blob.bin` and `userspace/initramfs.tar` are intentionally ignored exactly once without hiding unrelated source or sibling paths.
- [x] 3.2 (VIBIX) Verify generated staging outputs remain untracked/ignored with `git check-ignore` and verify no generated binaries are staged, committed, or pushed.
- [x] 3.3 (VIBIX validation) Compare sibling repository working-tree status before and after generation and confirm the VIBIX flow made no sibling source changes.

## 4. Boot and shell integration verification

- [x] 4.1 (VIBIX) Build a clean boot image with `make INIT=vibit` and confirm the produced image uses the VIBIT init payload rather than the default combined blob.
- [x] 4.2 (VIBIX) Add or update bounded serial QEMU harness checks for VIBIT/PID 1 startup, fork/exec handoff, vish prompt, deterministic command input/output, and the existing shell exit/repeat continuation semantics.
- [x] 4.3 (VIBIX) Run the integration test with an explicit timeout or success condition and return deterministic diagnostics for missing markers, early crash, or timeout.

## 5. Regression gates and completion

- [x] 5.1 (VIBIX) Run `make test` and preserve passing anti-plagiarism and kernel checks.
- [x] 5.2 (VIBIX) Run `make test_vibit` and confirm the existing VIBIT harness remains green.
- [ ] 5.3 (VIBIX) Review the final diff and status to ensure only planned VIBIX build/staging/ignore/test files and OpenSpec artifacts are included; generated binaries are absent from the change.
