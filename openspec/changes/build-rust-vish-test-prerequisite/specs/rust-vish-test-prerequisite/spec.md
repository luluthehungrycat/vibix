## ADDED Requirements

### Requirement: Rust vish probe builds its sibling ELF prerequisite
The VIBIX Rust-vish probe SHALL invoke the sibling repository's documented Rust ELF target before copying the ELF into the test initramfs.

#### Scenario: Clean sibling checkout
- **WHEN** `make test_vibit_rust` runs with `../vish` and its toolchain available
- **THEN** it invokes `make -C ../vish elf` before staging `/bin/vish`

#### Scenario: Existing artifact is stale
- **WHEN** an old Rust vish ELF exists before the probe
- **THEN** the sibling `elf` target still runs and its resulting artifact is staged

### Requirement: Rust vish artifact is validated before archive replacement
The probe SHALL verify that the expected Rust ELF exists and is non-empty before replacing `/bin/vish`.

#### Scenario: Artifact is available
- **WHEN** the sibling build succeeds and the artifact is non-empty
- **THEN** it is copied and the probe continues

#### Scenario: Artifact is unavailable
- **WHEN** the build fails or the artifact is missing or empty
- **THEN** the target stops before archive replacement and reports the prerequisite

### Requirement: Sibling source ownership remains separate
The VIBIX target SHALL consume the sibling build without editing or generating sibling source files.

#### Scenario: Probe stages Rust vish
- **WHEN** the prerequisite completes
- **THEN** only VIBIX-owned staging/archive outputs are changed
