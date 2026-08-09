## 1. Build prerequisite

- [x] 1.1 Invoke the sibling vish `elf` target from `test_vibit_rust` before artifact staging.
- [x] 1.2 Validate the expected non-empty Rust ELF and stop with an actionable diagnostic when unavailable.
- [x] 1.3 Preserve the existing temporary initramfs replacement and NASM VIBIT target behavior.

## 2. Validation

- [x] 2.1 Verify a clean Rust-vish probe builds the sibling artifact before copying it.
- [x] 2.2 Verify missing/failed artifact behavior stops before archive replacement.
- [x] 2.3 Run `make test_vibit_rust`, `make test_vibit`, and `make test`; the Rust probe now passes its strict entry/provenance/no-exception checks.
