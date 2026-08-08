## Context

VIBIX selects its init payload through `INIT` in the root and userspace Makefiles. The existing `INIT=vibit` path invokes `make -C ../vibit all`, copies the generated `vibit.bin` into the VIBIX init payload, invokes the sibling vish build, and assembles `userspace/initramfs.tar` with `/sbin/init` and `/bin/vish`. VIBIT is a flat NASM binary intended to run as PID 1; vish has a separate documented NASM flat target and an experimental Rust ELF target. The current repository also has `test_vibit` and serial QEMU harness behavior that must remain intact.

The change is constrained by `AGENTS.md`: sibling repositories own their source and their own PRs; VIBIX may stage generated binaries for development/testing only. The design therefore treats `../vibit` and `../vish` as build-time binary providers and keeps all integration edits in VIBIX planning scope.

## Goals / Non-Goals

**Goals:**

- Make the VIBIX `INIT=vibit` build reproducibly invoke documented sibling build targets, stage the generated VIBIT and NASM vish binaries, and assemble a bootable initramfs.
- Keep `/sbin/init` backed by VIBIT and `/bin/vish` backed by the NASM flat binary, preserving the existing non-ELF shell path.
- Define narrow, intentional ignore rules for generated VIBIX staging outputs and prevent generated binaries from being committed.
- Fail early with actionable diagnostics when sibling directories, NASM/toolchains, build outputs, or required staging inputs are unavailable.
- Verify PID 1 startup, VIBIT fork/exec of vish, prompt/input/exit behavior, and existing kernel regressions with bounded serial QEMU tests.

**Non-Goals:**

- No kernel syscall, scheduler, ELF loader, or interrupt changes.
- No source-code changes, generated-binary commits, or pushes in `../vibit`, `../vish`, or any other sibling repository.
- No replacement of the NASM vish path with the Rust ELF experiment; Rust ELF coverage remains a separate existing test path.
- No redesign of VIBIT lifecycle semantics or vish command semantics beyond integration acceptance checks.

## Decisions

1. **Sibling build ownership stays external.** VIBIX invokes `make -C ../vibit all` for VIBIT and `make -C ../vish nasm` for the documented flat vish binary. These targets remain the source of truth for generation; VIBIX copies only their output files. Alternatives considered: duplicating assembly or build rules in VIBIX (rejected because it violates repository ownership and risks drift), or relying on checked-in binaries (rejected because provenance and reproducibility become opaque).

2. **Use a temporary initramfs assembly directory.** The generated VIBIT output is staged as `userspace/vibix_blob.bin` for the kernel's existing include path, while the generated vish output is placed temporarily at `/bin/vish` during `userspace/initramfs.tar` creation. The temporary tree is removed after archiving. Alternatives considered: committing an extracted tree (rejected because it mixes generated artifacts with source) and embedding vish in the kernel blob (rejected because VIBIT must exec a separate process).

3. **Preserve the NASM integration contract.** `/bin/vish` is the sibling `vish.bin` flat binary, not `target/.../vibix`. The Rust ELF output may continue to be built or tested only where an existing experimental target explicitly requests it. This avoids changing the proven flat-binary execution path while leaving future ELF work isolated.

4. **Make ignore policy explicit and narrow.** Review the existing broad artifact patterns and retain/replace them with rules that cover only generated VIBIX outputs required by this flow, including `userspace/vibix_blob.bin` and `userspace/initramfs.tar`, without duplicate entries. General kernel build artifacts must remain ignored only where already required by the repository's build policy. Alternatives considered: adding another overlapping `userspace/*.bin` rule (rejected as overbroad) or relying on global `*.bin`/`*.tar` patterns alone (rejected because the staging contract is not visible and unrelated artifacts are hidden).

5. **Use bounded serial QEMU evidence.** Integration tests build with `INIT=vibit`, launch QEMU with a timeout, feed deterministic serial input, and assert markers for VIBIT/PID 1, fork/exec, vish prompt, command output, and exit/repeat behavior. Existing `make test` and `make test_vibit` remain required gates. Alternatives considered: an unbounded interactive `make run` (rejected for CI) and binary-only inspection (rejected because it cannot prove process handoff or TTY behavior).

## Risks / Trade-offs

- [Risk] Sibling repos or required toolchains are absent or stale -> Mitigation: validate directories, commands, output paths, and non-empty/generated artifacts before staging; emit the exact missing prerequisite and remediation command.
- [Risk] A stale staged binary masks a failed sibling build -> Mitigation: build first, check command status and output freshness/provenance, then copy; do not fall back silently to an existing artifact.
- [Risk] Temporary initramfs cleanup or archive ordering changes boot behavior -> Mitigation: create deterministic `/sbin/init` and `/bin/*` entries, inspect archive contents, and run the bounded QEMU marker test.
- [Risk] Narrowing `.gitignore` exposes unrelated existing build outputs -> Mitigation: compare `git status --ignored` before and after and preserve only intentional repository build exclusions while avoiding duplicate generated-user-space rules.
- [Risk] Shell timing makes prompt/input assertions flaky -> Mitigation: use explicit serial markers, bounded waits/timeouts, and the harness's existing exit semantics rather than arbitrary long sleeps.
- [Risk] VIBIT or vish source defects require sibling changes -> Mitigation: record the failure and open a separate PR in the owning sibling repository; do not patch sibling sources from VIBIX.

## Migration Plan

1. Confirm sibling repositories and required toolchains are available without changing either sibling.
2. Implement VIBIX-only build/staging and ignore-rule changes, then build `INIT=vibit` from a clean state.
3. Inspect generated artifacts and archive paths/provenance, run bounded QEMU integration checks, then run existing kernel and anti-plagiarism tests.
4. On failure, remove generated local artifacts with the normal clean target and revert only VIBIX integration edits; sibling repositories remain untouched.

## Open Questions

- Whether the existing harness already exposes stable VIBIT and vish marker strings or needs marker assertions added in VIBIX's test harness.
- Whether CI environments guarantee NASM, QEMU, and the `x86_64-unknown-none` Rust target, or need a documented prerequisite check before the integration target.
