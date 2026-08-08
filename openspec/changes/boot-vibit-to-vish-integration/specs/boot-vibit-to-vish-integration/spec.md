## ADDED Requirements

### Requirement: Reproducible sibling binary generation
The VIBIX integration build SHALL invoke the documented build targets in the sibling repositories before staging artifacts: `make -C ../vibit all` for VIBIT and the vish NASM target (`make -C ../vish nasm`) for the flat shell binary.

#### Scenario: Both sibling builds succeed
- **WHEN** the operator runs the VIBIX integration build with `INIT=vibit` from a clean state and both sibling repositories and toolchains are available
- **THEN** the build completes the VIBIT and NASM vish targets and exposes the generated `../vibit/vibit.bin` and `../vish/vish.bin` outputs for staging

#### Scenario: A prerequisite is missing
- **WHEN** a sibling repository, NASM/toolchain command, or expected generated output is missing
- **THEN** the build fails before producing a boot image and reports the missing path or command plus the owning repository and documented remediation command

### Requirement: Binary-only VIBIX staging
The integration SHALL copy only generated binaries into VIBIX staging locations and SHALL assemble `/sbin/init` from VIBIT and `/bin/vish` from the NASM flat vish binary without copying sibling source files.

#### Scenario: Initramfs contains the intended binaries
- **WHEN** `INIT=vibit` staging completes
- **THEN** `userspace/initramfs.tar` contains `/sbin/init` and `/bin/vish`, with `/sbin/init` sourced from generated VIBIT and `/bin/vish` sourced from generated `vish.bin`

#### Scenario: Source isolation is preserved
- **WHEN** the staged archive and VIBIX working tree are inspected after the build
- **THEN** no sibling source files are copied into VIBIX staging and no files under `../vibit` or `../vish` are modified by the VIBIX flow

### Requirement: Explicit generated-artifact ignore policy
VIBIX SHALL ignore the generated files produced for this integration, including the VIBIX init payload and initramfs archive, using narrow non-duplicated rules that do not hide arbitrary sibling or user source files.

#### Scenario: Generated outputs are ignored
- **WHEN** the integration build creates its generated VIBIX staging outputs
- **THEN** `git status` does not report those outputs as untracked files and `git check-ignore` identifies the intentional VIBIX-specific ignore rule

#### Scenario: Ignore policy is reviewed
- **WHEN** the repository ignore file is checked for this change
- **THEN** each generated staging output is covered exactly once by an intentional rule and no new catch-all rule for all binaries, archives, or sibling paths is introduced

### Requirement: Boot VIBIT as PID 1
The VIBIX build SHALL accept `INIT=vibit` and produce a bootable image whose first user process is the staged VIBIT binary at the existing user code location.

#### Scenario: Boot with VIBIT
- **WHEN** a clean `INIT=vibit` image is launched in QEMU
- **THEN** serial output shows VIBIT running as PID 1 and the kernel reaches the VIBIT boot sequence without falling back to the default combined blob

### Requirement: VIBIT launches the NASM vish shell
VIBIT SHALL fork and exec the staged `/bin/vish` process, and the integration SHALL preserve the existing NASM flat-binary execution path.

#### Scenario: Shell handoff succeeds
- **WHEN** VIBIT completes its boot sequence in the `INIT=vibit` image
- **THEN** bounded serial output contains stable markers proving fork, exec, and entry into the NASM vish shell

#### Scenario: Shell prompt and input work
- **WHEN** the bounded QEMU test sends a supported command through the serial input after the vish prompt appears
- **THEN** vish emits its prompt and deterministic command output, proving the TTY read/write path is usable

#### Scenario: Shell exits and init continues
- **WHEN** the bounded test sends the existing harness exit command or causes the shell child to exit
- **THEN** VIBIT follows its existing wait/repeat/reaper semantics, and the observed output matches the harness's documented continuation or termination marker

### Requirement: Regression and bounded validation
The integration SHALL provide bounded, non-interactive validation of the boot path and SHALL preserve the existing kernel and NASM VIBIT tests.

#### Scenario: Integration test is bounded
- **WHEN** the integration validation is run in CI without an interactive terminal
- **THEN** QEMU is terminated by a defined timeout or success condition and the test returns a deterministic pass/fail result from serial markers

#### Scenario: Existing tests remain green
- **WHEN** the normal kernel test suite and existing `make test_vibit` test are run after the integration changes
- **THEN** anti-plagiarism checks and existing kernel/VIBIT assertions pass without requiring generated binaries to be committed

### Requirement: Separate sibling ownership
VIBIX integration work SHALL modify only VIBIX build, staging, ignore, and test artifacts; any required VIBIT or vish source change SHALL be proposed and implemented separately in its owning sibling repository.

#### Scenario: No sibling source change is needed
- **WHEN** the VIBIX integration is implemented and validated
- **THEN** sibling working trees have no source modifications, generated binaries are not committed or pushed, and only VIBIX artifacts are eligible for the VIBIX change

#### Scenario: A sibling fix is discovered
- **WHEN** validation identifies a defect that requires changing VIBIT or vish source
- **THEN** the VIBIX work records the dependency and stops at the binary-provider boundary while a separate sibling-repository PR owns the source fix
