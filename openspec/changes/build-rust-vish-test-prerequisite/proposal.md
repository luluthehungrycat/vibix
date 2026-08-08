## Why

`make test_vibit_rust` copies the sibling Rust vish ELF but currently relies on a pre-existing Cargo artifact. A clean checkout can therefore fail before the probe starts even when the sibling repository is available.

## What Changes

- Invoke the existing sibling vish Rust ELF build target before staging `/bin/vish` for the Rust probe.
- Check that the expected ELF exists and is non-empty before archive replacement.
- Preserve the existing NASM vish integration path and provide clear non-interactive failure messages.
- Keep sibling source ownership unchanged; VIBIX only invokes the sibling build and copies its binary.

## Capabilities

### New Capabilities
- `rust-vish-test-prerequisite`: Makes the VIBIX Rust-vish probe reproducibly build and validate its sibling ELF prerequisite.

### Modified Capabilities

<!-- No existing capability specifications are present in the repository-local OpenSpec tree. -->

## Impact

- Affects the VIBIX Makefile and Rust-vish test staging only.
- Requires the existing `../vish` checkout and its `elf` build target; no sibling source modifications are allowed.
