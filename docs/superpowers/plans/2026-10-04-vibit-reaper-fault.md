# VIBIT Reaper Fault Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the VIBIT child-reap page fault and prove the shell respawns and accepts input.

**Architecture:** Keep the fix in VIBIT's decimal formatter. Strengthen the existing VIBIX QEMU integration assertion to require a numeric reaped PID, then run the complete serial interaction and respawn scenario.

**Tech Stack:** NASM, Python integration harness, VIBIX/QEMU TCG.

**Spec:** `../vibit/docs/superpowers/specs/2026-10-04-vibit-reaper-fault-design.md`

## Global Constraints

- Do not change the kernel syscall ABI or kernel code to mask the VIBIT pointer bug.
- Assemble into `/workspace/.onboarding` or ignored output paths; preserve tracked `vibit.bin` and `vibit.lst`.
- Use `/workspace/.onboarding/stage-vibit.sh` and QEMU TCG for reproducible serial integration.
- Record the integration result in `/workspace/vibix/CHANGELOG.md`.

## Review Focus

- `print_decimal` passes a buffer pointer to `strlen`; the existing integration test must fail before the fix and pass after it.
- Reaped PID output is nonempty and numeric, not just the fixed text prefix.
- Child exit reaches respawn and a later shell prompt.
- Ctrl-C at the respawned blocked reader still follows the expected kernel signal path.
- The VIBIX kernel boot probe continues to pass with VIBIT selected.

---

### Task 1: Reaper PID formatting and regression assertion

**Files:**
- Modify: `/workspace/vibit/vibit.asm`
- Modify: `/workspace/vibix/test_kernel.py`
- Modify: `/workspace/vibix/CHANGELOG.md`

**Interfaces:**
- Consumes: the existing `test_kernel.test_vibit_integration()` serial QEMU driver.
- Produces: a reaper transcript containing `VIBIT: reaped child <decimal PID>` followed by respawn and another prompt.

- [x] **Step 1: Strengthen the integration assertion first.** Require a match for `VIBIT: reaped child [0-9]+` in the serial transcript, while retaining the existing respawn and Ctrl-C checks.
- [x] **Step 2: Run the integration to verify RED.** Run `/workspace/.onboarding/stage-vibit.sh`, then call `test_kernel.test_vibit_integration()` from `/workspace/vibix` with `test_kernel.QEMU` set to `/workspace/.onboarding/bin/qemu-system-x86_64`.

Expected: FAIL after the transcript prints `VIBIT: reaped child ` and the page fault reports RIP `0x200049F`, CR2 `1`.

- [x] **Step 3: Correct the `strlen` argument in `print_decimal`.** In `vibit.asm`, set `rdi` to the digit-buffer pointer in `r13` immediately before calling `strlen`; retain fd 1 for the subsequent write syscall.
- [x] **Step 4: Run the integration to verify GREEN.** Rebuild through `/workspace/.onboarding/stage-vibit.sh` and rerun the same bounded integration.

Expected: numeric reaped PID, two respawn markers, three shell prompts, expected command output, and PASS.

- [x] **Step 5: Run the full VIBIT boot/integration check.** Run `test_kernel.test_vibit()` with the local QEMU wrapper.

Expected: all VIBIT boot markers and the interaction/respawn sequence PASS.

- [ ] **Step 6: Update the VIBIX changelog and commit the VIBIT/VIBIX files.** Record changed files and both passing checks.
