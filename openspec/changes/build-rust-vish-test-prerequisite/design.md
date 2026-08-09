## Context

`test_vibit_rust` copies `../vish/target/x86_64-unknown-none/release/vibix` but does not first invoke vish's documented `elf` target. Clean checkouts therefore depend on stale build artifacts.

## Goals / Non-Goals

**Goals:** invoke the sibling `elf` target, validate the exact non-empty artifact, and fail before archive replacement with a clear message.

**Non-Goals:** editing sibling source/build files, duplicating Cargo flags, or changing the ELF runtime/probe.

## Decisions

- Invoke `$(MAKE) -C ../vish elf`, keeping build policy in the sibling repository.
- Validate `target/x86_64-unknown-none/release/vibix` before copying.
- Keep this prerequisite local to `test_vibit_rust`; NASM VIBIT tests remain independent.

## Risks / Trade-offs

- [Missing sibling checkout] → report the expected path and target.
- [Changed sibling output path] → treat the documented path mismatch as an explicit compatibility failure.
- [Extra build time] → limit it to the Rust-specific probe.

## Migration Plan

No migration. Existing invocation becomes reproducible; rollback reverts only the VIBIX Makefile lines.

## Open Questions

Whether an offline prebuilt-artifact override is worth supporting later.
