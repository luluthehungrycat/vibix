#!/usr/bin/env python3
"""Test: run VIBIX kernel in QEMU and check for expected output."""
import subprocess
import sys
import os
import re
import tempfile
import selectors
import time
import socket
import struct
import shutil
import contextlib
import io
import signal
from dataclasses import dataclass
from pathlib import Path

QEMU = "/usr/bin/qemu-system-x86_64"


def test_exec_argv_vector_boundary_fixture_address():
    """Keep the vector-boundary fixture's first string pointer mapped."""
    source = Path(__file__).read_text()
    match = re.search(
        r"lea rsi, \[rsp-8\]\s+lea rcx, \[rel argv0\](.*?)mov \[rsi\], rcx",
        source,
        re.DOTALL,
    )
    assert match is not None, "argv vector-boundary fixture setup was not found"
    assert not re.search(r"\badd\s+rcx\b", match.group(1), re.IGNORECASE), (
        "RIP-relative argv0 already contains the flat-image address; do not add the image base"
    )


def _exec_argv_env_debug_evidence(output):
    """Require ordered, PID-correlated evidence for DEBUG-only ABI probes."""
    armed = re.search(r"DBG EXEC TEST: scheduled return armed pid=(\d+)\n", output)
    observed = re.search(r"DBG EXEC TEST: scheduled frame observed pid=(\d+)\n", output)
    progress = output.find("DBG EXEC TEST: failed image construction mapped_stack_pages=1\n")
    preserved = output.find("EXEC ABI IMAGE FAILURE PRESERVED\n")
    flat = output.find("EXEC ABI FLAT PASS\n")
    elf = output.find("EXEC ABI ELF PASS\n")
    schedule_ok = bool(
        armed and observed and armed.group(1) == observed.group(1)
        and armed.start() < observed.start() < flat < elf
    )
    failure_ok = progress >= 0 and preserved > progress
    return schedule_ok, failure_ok


def _exec_argv_env_debug_completion(output):
    """Stop the DEBUG probe on a terminal failure or a complete ELF PASS line."""
    terminal_failures = (
        "EXEC ABI IMAGE FAILURE FAIL",
        "EXEC ABI ELF FAIL",
        "EXEC ABI FLAT FAIL",
        "EXEC ABI RUNTIME FAIL",
        "VIBIX: EXCEPTION:",
        "VIBIX: PANIC:",
    )
    return any(marker in output for marker in terminal_failures) or "EXEC ABI ELF PASS\n" in output


def test_exec_argv_env_debug_evidence():
    """Reject late, mismatched, and progress-free DEBUG runtime evidence."""
    valid = (
        "DBG EXEC TEST: failed image construction mapped_stack_pages=1\n"
        "EXEC ABI IMAGE FAILURE PRESERVED\n"
        "DBG EXEC TEST: scheduled return armed pid=4\n"
        "DBG EXEC TEST: scheduled frame observed pid=4\n"
        "EXEC ABI FLAT PASS\nEXEC ABI ELF PASS\n"
    )
    assert not _exec_argv_env_debug_completion("EXEC ABI ELF PASS"), (
        "QEMU completion must wait until the ELF PASS line terminator arrives"
    )
    assert _exec_argv_env_debug_completion("EXEC ABI ELF PASS\n")
    assert _exec_argv_env_debug_evidence(valid) == (True, True)
    assert _exec_argv_env_debug_completion("EXEC ABI FLAT FAIL"), (
        "terminal runtime failures should still stop QEMU immediately"
    )
    assert not _exec_argv_env_debug_evidence(
        valid.replace("DBG EXEC TEST: scheduled frame observed pid=4\n", "")
        .replace("EXEC ABI FLAT PASS\n", "EXEC ABI FLAT PASS\n")
        .replace("EXEC ABI ELF PASS\n", "EXEC ABI ELF PASS\nDBG EXEC TEST: scheduled frame observed pid=4\n")
    )[0], "late scheduling observation must not pass"
    assert not _exec_argv_env_debug_evidence(valid.replace("observed pid=4", "observed pid=5"))[0], (
        "scheduled observation must match the armed exec PID"
    )
    assert not _exec_argv_env_debug_evidence(
        valid.replace("mapped_stack_pages=1\n", "mapped_stack_pages=0\n")
    )[1], "zero-page construction must not pass"
    assert not _exec_argv_env_debug_evidence(
        valid.replace("DBG EXEC TEST: failed image construction mapped_stack_pages=1\n", "")
    )[1], "missing construction progress must not pass"
def _test_process_group_probe_handles_zombie_leader_and_non_utf8_threads():
    """A dead thread-group leader must not hide a live worker thread."""
    with tempfile.TemporaryDirectory(prefix="vibix-proc-stat-fixture-") as temp_dir:
        proc_root = Path(temp_dir)
        process = proc_root / "700"
        task_dir = process / "task"
        task_dir.mkdir(parents=True)

        def write_stat(path, pid, comm, state):
            path.write_bytes(
                str(pid).encode() + b" (" + comm + b") " + state + b" 1 700 0\n"
            )

        write_stat(process / "stat", 700, b"lead\xff", b"Z")
        (task_dir / "700").mkdir()
        write_stat(task_dir / "700" / "stat", 700, b"lead\xff", b"Z")
        (task_dir / "701").mkdir()
        write_stat(task_dir / "701" / "stat", 701, b"worker\xfe", b"R")
        assert _process_group_has_live_members(700, proc_root=proc_root)

        write_stat(task_dir / "701" / "stat", 701, b"worker\xfe", b"Z")
        assert not _process_group_has_live_members(700, proc_root=proc_root)


def _process_group_has_live_members(process_group, proc_root=Path("/proc")):
    """Return whether procfs shows a live process or thread in this group."""
    for entry in proc_root.iterdir():
        if not entry.name.isdecimal():
            continue
        try:
            task_entries = list((entry / "task").iterdir())
        except (FileNotFoundError, PermissionError, ProcessLookupError):
            task_entries = []
        stat_paths = [task / "stat" for task in task_entries]
        if not stat_paths:
            stat_paths = [entry / "stat"]
        for stat_path in stat_paths:
            try:
                stat = stat_path.read_bytes()
            except (FileNotFoundError, PermissionError, ProcessLookupError):
                continue
            fields = stat.rsplit(b")", 1)[-1].split()
            if len(fields) > 2 and fields[2].isdigit() and int(fields[2]) == process_group:
                if fields[0] not in {b"Z", b"X"}:
                    return True
    return False


def _run_owned_process_group(command, *, cwd, timeout):
    """Run a build and stop every process in its private group before returning."""
    with tempfile.TemporaryFile() as stdout_file, tempfile.TemporaryFile() as stderr_file:
        proc = subprocess.Popen(
            command, cwd=cwd, stdout=stdout_file, stderr=stderr_file,
            start_new_session=True,
        )
        timed_out = False
        try:
            try:
                returncode = proc.wait(timeout=timeout)
            except subprocess.TimeoutExpired:
                timed_out = True
                try:
                    os.killpg(proc.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    proc.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    pass
                returncode = proc.returncode
        finally:
            # Always reap descendants before callers restore or remove build inputs.
            try:
                os.killpg(proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            if proc.poll() is None:
                proc.wait()
            while _process_group_has_live_members(proc.pid):
                try:
                    os.killpg(proc.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                time.sleep(0.01)
        stdout_file.seek(0)
        stderr_file.seek(0)
        stdout = stdout_file.read().decode(errors="replace")
        stderr = stderr_file.read().decode(errors="replace")
    if timed_out:
        raise subprocess.TimeoutExpired(command, timeout, output=stdout, stderr=stderr)
    return subprocess.CompletedProcess(command, returncode, stdout, stderr)


def _test_owned_process_group_cleanup():
    """Ensure a TERM-exiting leader cannot leave a live build descendant."""
    _test_process_group_probe_handles_zombie_leader_and_non_utf8_threads()
    with tempfile.TemporaryDirectory(prefix="vibix-process-group-test-") as temp_dir:
        pid_path = Path(temp_dir) / "child.pid"
        child_code = (
            "import signal,time; "
            "signal.signal(signal.SIGTERM, lambda *_: time.sleep(30)); "
            "time.sleep(30)"
        )
        leader_code = (
            "import os,subprocess,sys,time; "
            "child=subprocess.Popen([sys.executable,'-c',sys.argv[2]]); "
            "open(sys.argv[1],'w').write(str(os.getpid())+' '+str(child.pid)); "
            "time.sleep(30)"
        )
        command = [sys.executable, "-c", leader_code, str(pid_path), child_code]
        try:
            _run_owned_process_group(command, cwd=Path.cwd(), timeout=1)
        except subprocess.TimeoutExpired:
            pass
        else:
            return False
        if not pid_path.exists():
            return False
        process_group = int(pid_path.read_text().split()[0])
        # The leader's PID is its process-group ID; the group must be fully quiet.
        return not _process_group_has_live_members(process_group)


@dataclass
class ProbeResult:
    transcript: str
    accelerator: str | None
    attempt: int | None
    readiness: bool
    completion: bool
    classification: str
    reason: str


PROBE_FATAL_MARKERS = (
    "VIBIX: EXCEPTION:",
    "VIBIX: PANIC:",
    "kernel panic",
    "triple fault",
)


def _probe_has_fatal_evidence(transcript):
    lowered = transcript.lower()
    return any(marker.lower() in lowered for marker in PROBE_FATAL_MARKERS)


def _classify_probe(transcript, readiness, completion, qemu_error=""):
    """Classify product evidence before ranking transcript richness."""
    if not readiness:
        return "BLOCKED", qemu_error.strip() or "no serial readiness"
    if _probe_has_fatal_evidence(transcript):
        return "FAIL", "exception or panic evidence present"
    if completion:
        return "PASS", "completion predicate observed"
    return "FAIL", qemu_error.strip() or "required completion evidence missing"


def _probe_result_score(result):
    """Rank semantic outcome before completion/readiness/evidence richness."""
    classification_rank = {"BLOCKED": 0, "FAIL": 1, "PASS": 2}
    evidence = (
        int("VIBIT: reaper loop" in result.transcript)
        + result.transcript.count("DBG RUSTSCHED ")
    )
    return (
        classification_rank[result.classification],
        int(result.completion),
        int(result.readiness),
        evidence,
    )


def test_probe_selection():
    """Keep valid PASS attempts ahead of marker-rich failures and blocks."""
    passing = ProbeResult(
        "OK\nDBG RUSTSCHED TARGET pid=3",
        "kvm",
        1,
        True,
        True,
        "PASS",
        "completion predicate observed",
    )
    marker_rich_failure = ProbeResult(
        "VIBIT: reaper loop\n" + "DBG RUSTSCHED noise\n" * 8 + "VIBIX: EXCEPTION: #14\n",
        "tcg",
        1,
        True,
        True,
        "FAIL",
        "exception or panic evidence present",
    )
    blocked = ProbeResult("", "kvm", 2, False, False, "BLOCKED", "no serial readiness")
    assert _probe_result_score(passing) > _probe_result_score(marker_rich_failure)
    assert _probe_result_score(marker_rich_failure) > _probe_result_score(blocked)
    assert _classify_probe("OK\nVIBIX: PANIC: test", True, True)[0] == "FAIL"
    assert _classify_probe("", False, False)[0] == "BLOCKED"
    for fatal_marker in ("VIBIX: PANIC: test", "kernel panic", "triple fault"):
        assert _classify_probe("OK\n" + fatal_marker, True, True)[0] == "FAIL"
    assert _rust_probe_result_is_acceptable(passing, report=False)
    assert not _rust_probe_result_is_acceptable(marker_rich_failure, report=False)
    assert not _rust_probe_result_is_acceptable(blocked, report=False)
    assert not _rust_probe_result_is_acceptable(
        ProbeResult(
            "OK\nVIBIX: EXCEPTION: #14\n",
            "tcg",
            1,
            True,
            True,
            "PASS",
            "completion predicate observed",
        ),
        report=False,
    )
    for fatal_marker in ("VIBIX: PANIC: test", "kernel panic", "triple fault"):
        assert not _rust_probe_result_is_acceptable(
            ProbeResult(
                "OK\n" + fatal_marker,
                "tcg",
                1,
                True,
                True,
                "PASS",
                "completion predicate observed",
            ),
            report=False,
        )
    print("  ✅ Probe selection semantic ranking")
    return True


def _rust_probe_result_is_acceptable(probe, report=True):
    """Reject non-PASS or fatal selected transcripts before Rust checks run."""
    if probe.classification != "PASS":
        if report:
            print(
                f"  ❌ Rust ELF probe rejected: status={probe.classification} "
                f"reason={probe.reason}"
            )
        return False
    if _probe_has_fatal_evidence(probe.transcript):
        if report:
            print("  ❌ Rust ELF probe rejected: exception or panic evidence present")
        return False
    return True


CHECKLIST = {
    "VIBIX: Kernel alive!":               "Kernel boot",
    "PMM: Test passed.":                  "PMM self-test",
    "KMM: Test OK — freed block reused.":  "Heap allocator (reuse)",
    "KMM: Coalescing OK — single free block.": "Heap allocator (coalescing)",
    "PAGING: Test passed.":               "Paging self-test",
    "VIBIX: IDT loaded":                  "IDT initialization",
    "VIBIX: PIT timer initialised":       "PIT timer init",
    "VIBIX: Loading GDT/TSS":             "GDT/TSS and SYSCALL setup",
    "VIBIX: Enabling interrupts.":        "Interrupts enabled",
    "VIBIX: Boot sequence complete":      "Boot sequence",
    "VIBIX: Created PID 1 (init).":       "Init process creation",
    "VIBIX: Starting scheduler...":       "Scheduler start",
    "Hello, world!":                      "User-mode init output",
    "From PID 1 (init)":                  "Init process message",
    "VIBIX: PID 1 exited with code 0":   "Init exit",
    # Userspace syscall tests
    "User Test Begin":                    "Userspace test suite start",
    "  [OK] pipe test: HELLO_PIPE":       "Pipe syscall test",
    "  [OK] dup test: newfd=":           "Dup syscall test",
    "  [OK] dup2 test: newfd=5":         "Dup2 syscall test",
    "  [OK] getcwd test: ":              "Getcwd syscall test",
    "  [OK] fd bounds/closed semantics": "Fd bounds and closed-descriptor semantics",
    "  [TRACE] fd helper second-failure path": "FD helper failure-path reached",
    "  [OK] fd helper second-failure cleanup": "FD helper failure-path cleanup",
    "  [OK] VFS/legacy fd errors":       "VFS and legacy fd error conventions",
    "  [OK] lseek/getdents/TTY fd errors": "VFS lseek/getdents/TTY fd errors",
    "  [OK] dup2 replacement/refcount":  "Dup2 replacement and refcount survival",
    "User Test End":                      "Userspace test suite end",
}

OPTIONAL = {
    "VIBIX: Multiboot memory map:":       "Multiboot memory map",
    "VIBIX: EXCEPTION:":                  "Exception handler (future)",
    # Signal tests (optional -- may hang on boot configs without signal delivery)
    "Signal Test Begin":                  "Signal test suite start",
    "  [SIGNAL] caught SIGUSR1":         "Custom SIGUSR1 handler delivery",
    "  [OK] custom handler test":        "Custom handler survival check",
    "  [OK] ignore test":                "SIG_IGN test",
    "Signal Test End":                    "Signal test suite end",
}


def verify_serial(serial_output):
    """Run all checks against serial output. Returns (ok, failure_count)."""
    all_ok = True
    failed = 0
    for marker, label in CHECKLIST.items():
        if marker in serial_output:
            print(f"  ✅ {label}")
        else:
            print(f"  ❌ {label} (missing: {marker!r})")
            all_ok = False
            failed += 1
    for marker, label in OPTIONAL.items():
        if marker in serial_output:
            print(f"  ◇ {label}")
        else:
            print(f"  · {label} (absent)")
    return all_ok, failed


def test_kernel_alive():
    print("🧪 Testing VIBIX kernel...")

    with tempfile.NamedTemporaryFile(mode="w+", suffix=".serial", delete=False) as f:
        serial_path = f.name

    # Try KVM if available and usable, else fall back to TCG
    accel = "kvm"
    try:
        with open("/dev/kvm", "rb"):
            pass
    except (FileNotFoundError, PermissionError, OSError):
        accel = "tcg"

    try:
        result = subprocess.run(
            [
                QEMU,
                "-accel", accel,
                "-kernel", "vibix.elf",
                "-serial", f"file:{serial_path}",
                "-display", "none",
                "-m", "512M",
                "-no-reboot",
                "-no-shutdown",
            ],
            capture_output=True,
            text=True,
            timeout=30,
        )

        with open(serial_path, "r", errors="replace") as f:
            serial_output = f.read()

        ok, _ = verify_serial(serial_output)
        if ok:
            print("✅ All boot sequence checks passed.")
            return True
        else:
            print("❌ Some checks failed.")
            print("Full serial output:", repr(serial_output))
            return False

    except subprocess.TimeoutExpired:
        if os.path.exists(serial_path):
            with open(serial_path, "r", errors="replace") as f:
                serial_output = f.read()
            ok, failed = verify_serial(serial_output)
            if ok:
                print("✅ Test passed: all checks passed.")
                return True
            else:
                print(f"❌ Test failed: {failed} check(s) failed.")
                print("Full serial output:", repr(serial_output))
                return False
        print("❌ Test failed: QEMU timed out (kernel likely hung).")
        return False
    except Exception as e:
        print(f"❌ Test failed with error: {e}")
        return False
    finally:
        if os.path.exists(serial_path):
            os.unlink(serial_path)


# ── VIBIT init system markers ─────────────────────────────────────────────────
VIBIT_CHECKLIST = {
    "VIBIT v0.2.0: PID 1 init":       "VIBIT banner",
    "VIBIT: init task complete":       "Init task",
    "VIBIT: spawning shell":           "Shell spawn (fork)",
    "VIBIT: reaper loop":              "Reaper loop (waitpid)",
    "VIBIT: starting shell":           "Shell start",
    "vish -- VIBIX SHell":             "vish exec",
    "vish$ ":                          "Shell prompt (blocking read)",
}


def _vibit_boot_complete(serial_output):
    """Require every bounded VIBIT boot marker before declaring completion."""
    return all(marker in serial_output for marker in VIBIT_CHECKLIST)


def _print_selected_probe(label, probe, include_transcript=False):
    """Print deterministic identity and reason for a selected bounded probe."""
    print(
        f"  · {label} accelerator={probe.accelerator or 'none'} "
        f"attempt={probe.attempt or 'none'} "
        f"readiness={'yes' if probe.readiness else 'no'} "
        f"completion={'yes' if probe.completion else 'no'} "
        f"status={probe.classification} reason={probe.reason}"
    )
    if include_transcript and probe.classification != "PASS":
        print(f"  {label} transcript: {probe.transcript!r}")


def test_vibit():
    """Test the kernel with VIBIT init system (make INIT=vibit)."""
    print("🧪 Testing VIBIT init system...")

    with tempfile.NamedTemporaryFile(mode="w+", suffix=".serial", delete=False) as f:
        serial_path = f.name

    probe = _run_bounded_qemu(
        serial_path,
        _available_accelerators(),
        attempts=2,
        completion_predicate=_vibit_boot_complete,
        timeout=30,
    )
    serial_output = probe.transcript
    _print_selected_probe("VIBIT boot probe", probe, include_transcript=True)

    all_ok = True
    for marker, label in VIBIT_CHECKLIST.items():
        if marker in serial_output:
            print(f"  ✅ {label}")
        else:
            print(f"  ❌ {label} (missing: {marker!r})")
            all_ok = False

    integration_ok = test_vibit_integration()
    os.unlink(serial_path)
    return probe.classification == "PASS" and all_ok and integration_ok


def test_vibit_integration():
    """Exercise vish input and VIBIT shell respawn with bounded serial I/O."""
    print("🧪 Testing bounded VIBIT/vish shell handoff...")
    accel = "kvm"
    try:
        with open("/dev/kvm", "rb"):
            pass
    except (FileNotFoundError, PermissionError, OSError):
        accel = "tcg"

    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]

    command = [
        QEMU,
        "-accel", accel,
        "-kernel", "vibix.elf",
        "-serial", f"tcp:127.0.0.1:{port},server=on,wait=on",
        "-display", "none",
        "-m", "512M",
        "-no-reboot",
        "-no-shutdown",
    ]
    # DEBUG=1 emits verbose scheduler traces. Capturing these streams without
    # draining them can fill the OS pipe and stall QEMU before serial input is
    # serviced; this probe only consumes the serial socket.
    proc = subprocess.Popen(command, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    connection = None
    output = bytearray()
    sent_sigint = False
    observed_sigint_respawn = False
    sent_input = False
    sent_exit = False
    sent_respawn_help = False
    prompt_count = 0
    initial_idle_checked = False
    pre_exit_alive_checked = False
    readiness_observed = False
    terminal_reason = "deadline expired before completion"
    deadline = time.monotonic() + 30
    try:
        while connection is None and time.monotonic() < deadline:
            try:
                connection = socket.create_connection(("127.0.0.1", port), timeout=0.2)
            except OSError:
                if proc.poll() is not None:
                    break
        if connection is None:
            probe = ProbeResult(
                "", accel, 1, False, False, "BLOCKED",
                "serial TCP connection was not established",
            )
            _print_selected_probe("VIBIT/vish integration", probe, include_transcript=True)
            return False
        connection.setblocking(False)
        selector = selectors.DefaultSelector()
        selector.register(connection, selectors.EVENT_READ)
        initial_prompt_at = None
        input_complete_at = None
        respawn_help_complete_at = None
        reaped_pids = []
        reaped_count = 0
        while time.monotonic() < deadline:
            events = selector.select(min(0.05, max(0, deadline - time.monotonic())))
            for key, _ in events:
                chunk = key.fileobj.recv(4096)
                if not chunk:
                    terminal_reason = "serial connection closed"
                    deadline = time.monotonic()
                    break
                output.extend(chunk)
            text = output.decode("utf-8", errors="replace")
            now = time.monotonic()
            prompt_count = text.count("vish$ ")
            readiness_observed = readiness_observed or prompt_count > 0
            if prompt_count and initial_prompt_at is None:
                initial_prompt_at = now
            reaped_pids = re.findall(r"VIBIT: reaped child ([0-9]+)", text)
            reaped_count = len(re.findall(r"VIBIT: reaped child", text))

            if "VIBIT: respawn failed" in text:
                terminal_reason = "VIBIT could not load the shell ELF"
                break
            if not sent_exit and reaped_count != 0:
                terminal_reason = "shell was reaped before explicit exit"
                break
            if sent_exit and not sent_sigint and reaped_count > 1:
                terminal_reason = "shell was reaped more than once before Ctrl-C"
                break
            if sent_sigint and reaped_count > 2:
                terminal_reason = "more than two shells were reaped"
                break

            # Run phase transitions on every poll, including quiet periods
            # where QEMU emits no serial bytes while the shell blocks in read.
            if (not initial_idle_checked and initial_prompt_at is not None
                    and now - initial_prompt_at >= 0.5):
                initial_idle_checked = True
                connection.sendall(b"help\n")
                sent_input = True
            if (sent_input and not pre_exit_alive_checked
                    and "Built-in commands:" in text and prompt_count >= 2):
                pre_exit_alive_checked = True
                input_complete_at = now
            if (pre_exit_alive_checked and not sent_exit
                    and now - input_complete_at >= 0.25):
                connection.sendall(b"exit\n")
                sent_exit = True
            if sent_exit and reaped_count == 1:
                observed_sigint_respawn = "VIBIT: respawning shell..." in text
            if (observed_sigint_respawn and not sent_respawn_help
                    and prompt_count >= 3):
                connection.sendall(b"help\n")
                sent_respawn_help = True
            if (sent_respawn_help and respawn_help_complete_at is None
                    and text.count("Built-in commands:") >= 2 and prompt_count >= 4):
                respawn_help_complete_at = now
            if (respawn_help_complete_at is not None and not sent_sigint
                    and now - respawn_help_complete_at >= 0.25):
                if reaped_count != 1:
                    terminal_reason = "respawned shell exited before Ctrl-C"
                    break
                # The respawned shell is blocked in read; Ctrl-C must target it.
                connection.sendall(b"\x03")
                sent_sigint = True
            if (sent_sigint and reaped_count == 2 and prompt_count >= 5
                    and text.count("VIBIT: respawning shell...") >= 2):
                terminal_reason = "required prompt/respawn sequence observed"
                break
        selector.close()
        text = output.decode("utf-8", errors="replace")
        checks = {
            "Built-in commands:": "deterministic vish command output",
            "VIBIT: respawning shell...": "VIBIT shell continuation",
        }
        reaped_pid_observed = bool(reaped_pids)
        print(f"  {'✅' if reaped_pid_observed else '❌'} VIBIT child reaping")
        ok = (initial_idle_checked and pre_exit_alive_checked and sent_input
              and sent_exit and sent_respawn_help and sent_sigint
              and observed_sigint_respawn and reaped_count == 2
              and len(reaped_pids) == 2 and prompt_count >= 5
              and text.count("VIBIT: respawning shell...") >= 2)
        if not readiness_observed:
            print(f"  BLOCKED: required vish prompt never appeared (accelerator={accel}, attempt=1)")
        elif not ok:
            print(f"  FAIL: VIBIT/vish sequence incomplete ({terminal_reason})")
        for marker, label in checks.items():
            if marker in text:
                print(f"  ✅ {label}")
            else:
                print(f"  ❌ {label} (missing: {marker!r})")
                ok = False
        if len(reaped_pids) != 2:
            print(f"  ❌ expected two numeric child PIDs, observed {len(reaped_pids)}")
            ok = False
        if prompt_count >= 5:
            print("  ✅ vish prompt returned after shell exit")
        else:
            print(f"  ❌ vish prompt repetition (observed {prompt_count} prompt(s))")
        if not sent_sigint:
            print("  ❌ Ctrl-C was not sent (prompt missing)")
        if not observed_sigint_respawn:
            print("  ❌ shell exit/Ctrl-C did not produce the expected respawn")
        if not sent_input:
            print("  ❌ deterministic input was not sent (post-Ctrl-C prompt missing)")
        if not sent_exit:
            print("  ❌ exit command was not sent (command output missing)")
        classification, reason = _classify_probe(
            text, readiness_observed, ok, terminal_reason
        )
        probe = ProbeResult(
            text, accel, 1, readiness_observed, ok, classification, reason
        )
        _print_selected_probe("VIBIT/vish integration", probe, include_transcript=False)
        if not ok:
            excerpts = [text[:2500]]
            for marker in ("VIBIT: starting shell", "DBG EXEC: path=/bin/vish", "vish --", "vish$ ", "DBG tty:", "vish:"):
                index = text.find(marker)
                if index >= 0:
                    excerpts.append(text[max(0, index - 100):index + 1200])
            excerpts.append(text[-2500:])
            print("  Serial transcript excerpts:", repr("\n…\n".join(excerpts)))
        return classification == "PASS"
    finally:
        if connection is not None:
            connection.close()
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=2)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=2)


if __name__ == "__main__":
    sys.exit(0 if test_kernel_alive() else 1)


def _available_accelerators():
    """Return usable accelerator order without sleeping or guessing readiness."""
    try:
        with open("/dev/kvm", "rb"):
            return ["kvm", "tcg"]
    except (FileNotFoundError, PermissionError, OSError):
        return ["tcg"]


def _run_qemu_until_marker(serial_path, accelerator, markers, timeout=30,
                           completion_predicate=None):
    """Run QEMU until markers/predicate are visible, then terminate cleanly."""
    command = [
        QEMU, "-accel", accelerator, "-kernel", "vibix.elf",
        "-serial", f"file:{serial_path}", "-display", "none",
        "-m", "512M", "-no-reboot", "-no-shutdown",
    ]
    proc = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    deadline = time.monotonic() + timeout
    output = ""
    selector = selectors.DefaultSelector()
    for stream in (proc.stdout, proc.stderr):
        if stream is not None:
            selector.register(stream, selectors.EVENT_READ)
    try:
        while time.monotonic() < deadline:
            try:
                with open(serial_path, "r", errors="replace") as serial:
                    output = serial.read()
            except FileNotFoundError:
                output = ""
            complete = (
                completion_predicate(output)
                if completion_predicate is not None
                else any(marker in output for marker in markers)
            )
            if complete:
                proc.terminate()
                try:
                    proc.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    proc.wait(timeout=2)
                with open(serial_path, "r", errors="replace") as serial:
                    return serial.read(), ""
            if proc.poll() is not None:
                break
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                break
            for key, _ in selector.select(timeout=min(0.01, remaining)):
                try:
                    os.read(key.fd, 4096)
                except OSError:
                    pass
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=2)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=2)
        with open(serial_path, "r", errors="replace") as serial:
            output = serial.read()
        return output, "QEMU marker timeout"
    finally:
        selector.close()
        if proc.poll() is None:
            proc.kill()
            proc.wait(timeout=2)


def _run_bounded_qemu(serial_path, accelerators, attempts=2, completion_markers=None,
                      completion_predicate=None, timeout=30):
    """Run bounded probes and return a structured selected-attempt result."""
    best = ProbeResult(
        "", None, None, False, False, "BLOCKED",
        "no accelerator attempt produced serial evidence",
    )
    best_score = (-1, -1, -1, -1)
    markers = completion_markers or ("OK\n", "OK\r\n")
    for accelerator in accelerators:
        for attempt in range(attempts):
            open(serial_path, "w").close()
            if completion_markers or completion_predicate is not None:
                output, qemu_error = _run_qemu_until_marker(
                    serial_path,
                    accelerator,
                    completion_markers or (),
                    timeout=timeout,
                    completion_predicate=completion_predicate,
                )
            else:
                try:
                    completed = subprocess.run(
                        [QEMU, "-accel", accelerator, "-kernel", "vibix.elf",
                         "-serial", f"file:{serial_path}", "-display", "none",
                         "-m", "512M", "-no-reboot", "-no-shutdown"],
                        capture_output=True, text=True, timeout=30,
                    )
                    qemu_error = completed.stderr
                except subprocess.TimeoutExpired as exc:
                    qemu_error = str(exc)
            with open(serial_path, "r", errors="replace") as f:
                output = f.read()
            if qemu_error and not output:
                print(f"  · {accelerator} attempt {attempt + 1}: no serial readiness ({qemu_error.strip()[:120]})")
            completion_observed = (
                completion_predicate(output)
                if completion_predicate is not None
                else any(marker in output for marker in markers)
            )
            readiness_observed = bool(output)
            classification, reason = _classify_probe(
                output,
                readiness_observed,
                completion_observed,
                qemu_error,
            )
            result = ProbeResult(
                transcript=output,
                accelerator=accelerator,
                attempt=attempt + 1,
                readiness=readiness_observed,
                completion=completion_observed,
                classification=classification,
                reason=reason,
            )
            print(
                f"  · probe accelerator={accelerator} attempt={attempt + 1} "
                f"readiness={'yes' if readiness_observed else 'no'} "
                f"completion={'yes' if completion_observed else 'no'} status={classification}"
            )
            score = _probe_result_score(result)
            if score > best_score:
                best_score = score
                best = result
            if result.classification == "PASS" and completion_observed:
                break
        if best.accelerator == accelerator and best.classification == "PASS":
            break
    print(
        f"  · selected transcript accelerator={best.accelerator or 'none'} "
        f"attempt={best.attempt or 'none'} status={best.classification} reason={best.reason}"
    )
    return best


RUST_SCHED_REQUIRED_ROUND_TRIPS = 3
RUST_SCHED_PREFIX = "DBG RUSTSCHED "


def _parse_scheduler_fields(payload):
    fields = {}
    for token in payload.split():
        if "=" not in token:
            raise ValueError(f"malformed scheduler token: {token!r}")
        key, value = token.split("=", 1)
        if not key or key in fields:
            raise ValueError(f"duplicate/empty scheduler field: {key!r}")
        if value.startswith("0x"):
            fields[key] = int(value, 16)
        elif value.isdigit():
            fields[key] = int(value, 10)
        else:
            fields[key] = value
    return fields


def _is_canonical_user_address(value):
    if not isinstance(value, int) or value == 0 or value >= (1 << 64):
        return False
    upper = value >> 48
    sign = (value >> 47) & 1
    return upper == (0xffff if sign else 0)


def _rip_in_target_intervals(rip, intervals):
    return any(lo <= rip < hi for lo, hi in intervals)


def _parse_target_intervals(target):
    count = target.get("interval_count")
    if not isinstance(count, int) or count <= 0:
        raise ValueError("target interval_count must be positive")
    intervals = []
    for index in range(count):
        lo = target.get(f"interval{index}_lo")
        hi = target.get(f"interval{index}_hi")
        if not isinstance(lo, int) or not isinstance(hi, int) or lo >= hi:
            raise ValueError(f"invalid target interval {index}")
        if intervals and lo <= intervals[-1][1]:
            raise ValueError(f"target intervals overlap or are unsorted at {index}")
        intervals.append((lo, hi))
    return intervals


def _check_scheduler_interval_fixture():
    """Exercise gap rejection and a real 20-segment executable fixture."""
    image = _synthetic_exec_interval_elf(20)
    base = struct.unpack_from("<Q", image, 24)[0]
    phoff = struct.unpack_from("<Q", image, 32)[0]
    phentsize = struct.unpack_from("<H", image, 54)[0]
    phnum = struct.unpack_from("<H", image, 56)[0]
    intervals = []
    for index in range(phnum):
        offset = phoff + index * phentsize
        vaddr = struct.unpack_from("<Q", image, offset + 16)[0]
        memsz = struct.unpack_from("<Q", image, offset + 40)[0]
        intervals.append((vaddr, vaddr + memsz))
    target = {"interval_count": len(intervals)}
    for index, (lo, hi) in enumerate(intervals):
        target[f"interval{index}_lo"] = lo
        target[f"interval{index}_hi"] = hi
    parsed = _parse_target_intervals(target)
    return (
        base == 0x02000000
        and len(parsed) == 20
        and _rip_in_target_intervals(intervals[0][0], parsed)
        and _rip_in_target_intervals(intervals[-1][0], parsed)
        and not _rip_in_target_intervals(intervals[0][1] + 1, parsed)
    )


def _synthetic_exec_interval_elf(segment_count):
    """Build an ELF containing separated executable PT_LOAD segments."""
    base = 0x02000000
    phoff = 64
    phentsize = 56
    image = bytearray(phoff + phentsize * segment_count)
    image[:64] = struct.pack(
        "<16sHHIQQQIHHHHHH",
        b"\x7fELF" + bytes([2, 1, 1]) + bytes(9),
        2, 0x3E, 1, base, phoff, 0, 0, 64, phentsize,
        segment_count, 0, 0, 0,
    )
    for index in range(segment_count):
        vaddr = base + index * 0x2000
        struct.pack_into(
            "<IIQQQQQQ",
            image,
            phoff + index * phentsize,
            1, 5, 0, vaddr, vaddr, 1, 0x100, 0x1000,
        )
    return bytes(image)


def _validate_scheduler_frame(fields, target, label, require_kernel_rsp=False):
    required = {"pid", "cr3", "rip", "cs", "ss", "rflags", "rsp"}
    missing = sorted(required - fields.keys())
    if missing:
        return f"{label} missing fields: {', '.join(missing)}"
    if fields["pid"] != target["pid"]:
        return f"{label} PID mismatch: {fields['pid']} != {target['pid']}"
    if fields["cr3"] != target["cr3"]:
        return f"{label} CR3 mismatch: 0x{fields['cr3']:x} != 0x{target['cr3']:x}"
    if not _rip_in_target_intervals(fields["rip"], target["intervals"]):
        return f"{label} RIP outside target intervals: 0x{fields['rip']:x}"
    if fields["cs"] != 0x23 or fields["ss"] != 0x1B:
        return f"{label} user selectors invalid: cs=0x{fields['cs']:x} ss=0x{fields['ss']:x}"
    if fields["rflags"] & 0x200 == 0:
        return f"{label} IF flag missing: 0x{fields['rflags']:x}"
    if not _is_canonical_user_address(fields["rsp"]):
        return f"{label} user RSP invalid: 0x{fields['rsp']:x}"
    if require_kernel_rsp:
        if "krsp" not in fields or not _is_canonical_user_address(fields["krsp"]):
            return f"{label} kernel frame pointer invalid"
    return None


def _check_rust_elf_scheduler_evidence(serial_output, required_round_trips=RUST_SCHED_REQUIRED_ROUND_TRIPS):
    """Validate PID/CR3/RIP-correlated Rust ELF scheduler round trips."""
    target = None
    events = []
    parse_errors = []
    for line in serial_output.splitlines():
        if line.startswith(f"{RUST_SCHED_PREFIX}TARGET "):
            try:
                parsed = _parse_scheduler_fields(line[len(f"{RUST_SCHED_PREFIX}TARGET "):])
            except ValueError as exc:
                parse_errors.append(str(exc))
                continue
            if target is not None:
                parse_errors.append("duplicate Rust scheduler target record")
            target = parsed
        elif line.startswith(f"{RUST_SCHED_PREFIX}TARGET_") or line.startswith(f"{RUST_SCHED_PREFIX}CONFLICT"):
            parse_errors.append(f"invalid Rust scheduler target record: {line}")
        elif line.startswith(f"{RUST_SCHED_PREFIX}IN "):
            kind = "IN"
            payload = line[len(f"{RUST_SCHED_PREFIX}IN "):]
            try:
                events.append((kind, _parse_scheduler_fields(payload)))
            except ValueError as exc:
                parse_errors.append(str(exc))
        elif line.startswith(f"{RUST_SCHED_PREFIX}OUT "):
            kind = "OUT"
            payload = line[len(f"{RUST_SCHED_PREFIX}OUT "):]
            try:
                events.append((kind, _parse_scheduler_fields(payload)))
            except ValueError as exc:
                parse_errors.append(str(exc))
        elif line.startswith(f"{RUST_SCHED_PREFIX}USER "):
            kind = "USER"
            payload = line[len(f"{RUST_SCHED_PREFIX}USER "):]
            try:
                events.append((kind, _parse_scheduler_fields(payload)))
            except ValueError as exc:
                parse_errors.append(str(exc))

    if target is None:
        parse_errors.append("missing Rust scheduler target record")
    else:
        required_target = {"pid", "cr3", "interval_count", "entry"}
        missing = sorted(required_target - target.keys())
        if missing:
            parse_errors.append(f"target record missing fields: {', '.join(missing)}")
        else:
            try:
                target["intervals"] = _parse_target_intervals(target)
            except ValueError as exc:
                parse_errors.append(str(exc))
            if "intervals" in target and not _rip_in_target_intervals(target["entry"], target["intervals"]):
                parse_errors.append("target entry is outside executable intervals")

    if parse_errors:
        for error in parse_errors:
            print(f"  ❌ Rust scheduler evidence: {error}")
        return False
    if not events:
        print("  ❌ Rust scheduler evidence: no correlated events")
        return False

    completed = 0
    state = "need_in"
    last_seq = 0
    errors = []
    for kind, fields in events:
        required = {"seq", "round"}
        missing = sorted(required - fields.keys())
        if missing:
            errors.append(f"{kind} missing fields: {', '.join(missing)}")
            continue
        if fields["seq"] != last_seq + 1:
            errors.append(f"event sequence gap: expected {last_seq + 1}, got {fields['seq']}")
        last_seq = fields["seq"]
        if fields["round"] < completed:
            errors.append(f"round counter regressed at seq {fields['seq']}")

        if kind == "IN":
            error = _validate_scheduler_frame(fields, target, "SCHED_IN", require_kernel_rsp=True)
            if error:
                errors.append(error)
                continue
            source = fields.get("source")
            if state == "need_in" and source == "sysret" and completed == 0:
                if fields.get("prev") != 0 or fields["round"] != 0:
                    errors.append("initial SYSRET handoff has invalid predecessor or round")
                state = "need_user"
            elif state == "need_in" and source == "sched":
                if fields.get("prev") == target["pid"]:
                    errors.append("same-process SCHED_IN cannot start a round trip")
                else:
                    completed += 1
                    if fields["round"] != completed:
                        errors.append(f"SCHED_IN round mismatch: {fields['round']} != {completed}")
                    state = "need_user"
            elif state == "same_process_in" and source == "sched":
                if fields.get("prev") != target["pid"]:
                    errors.append("same-process continuation has wrong predecessor")
                state = "need_user"
            else:
                errors.append(f"unexpected SCHED_IN source/state: {source}/{state}")
        elif kind == "USER":
            error = _validate_scheduler_frame(fields, target, "USER_RETURN")
            if error:
                errors.append(error)
                continue
            if state != "need_user":
                errors.append(f"USER_RETURN out of order in state {state}")
            elif fields["round"] != completed:
                errors.append(f"USER_RETURN round mismatch: {fields['round']} != {completed}")
            else:
                state = "need_out"
        elif kind == "OUT":
            error = _validate_scheduler_frame(fields, target, "SCHED_OUT", require_kernel_rsp=True)
            if error:
                errors.append(error)
                continue
            if state != "need_out":
                errors.append(f"SCHED_OUT out of order in state {state}")
            elif fields["round"] != completed:
                errors.append(f"SCHED_OUT round mismatch: {fields['round']} != {completed}")
            elif fields.get("next") == target["pid"]:
                state = "same_process_in"
            else:
                state = "need_in"
        else:
            errors.append(f"unknown scheduler event: {kind}")

    if completed < required_round_trips:
        errors.append(
            f"round-trip threshold not met: observed {completed}, need {required_round_trips}"
        )
    if state == "need_out":
        errors.append("target returned to user but no target schedule-out was observed")
    if errors:
        for error in errors:
            print(f"  ❌ Rust scheduler evidence: {error}")
        print(f"  · Correlated event records: {len(events)}, completed round trips: {completed}")
        return False

    print(
        f"  ✅ Rust scheduler evidence: PID {target['pid']} completed "
        f"{completed} correlated round trips (threshold {required_round_trips})"
    )
    print(
        f"  ✅ PID/CR3/RIP evidence: cr3=0x{target['cr3']:x}, "
        f"intervals={target['intervals']}, events={len(events)}"
    )
    return True


def _scheduler_probe_complete(serial_output):
    """Require entry output and valid scheduler evidence before completion."""
    if _probe_has_fatal_evidence(serial_output):
        return False
    if "OK\n" not in serial_output and "OK\r\n" not in serial_output:
        return False
    with contextlib.redirect_stdout(io.StringIO()):
        return _check_rust_elf_scheduler_evidence(serial_output)


def _print_rust_elf_result(serial_output, require_shell_marker=False):
    required = {
        "VIBIT v0.2.0: PID 1 init": "VIBIT banner",
        "VIBIT: init task complete": "Init task",
        "VIBIT: reaper loop": "Reaper loop (waitpid)",
    }
    all_ok = True
    for marker, label in required.items():
        if marker in serial_output:
            print(f"  ✅ {label}")
        else:
            print(f"  ❌ {label} (missing: {marker!r})")
            all_ok = False

    if "VIBIT: spawning shell" in serial_output:
        print("  ✅ Shell spawn (fork)")
    elif require_shell_marker:
        print("  ❌ Shell spawn (fork) (missing early marker)")
        all_ok = False
    elif "DBG EXEC:" in serial_output or "OK\n" in serial_output:
        print("  ◇ Shell spawn marker absent, but later ELF readiness proves the kernel progressed")
    else:
        print("  ❌ Shell spawn marker absent and no later ELF readiness")
        all_ok = False

    evidence_lines = [
        line for line in serial_output.splitlines()
        if line.startswith("DBG EXEC:")
        or line.startswith("DBG ELF:")
        or line.startswith("DBG ELF COPY:")
        or line.startswith("DBG ELF WALK:")
        or line.startswith("DBG ELF META:")
        or line.startswith("VIBIX: PF_CLASS:")
    ]
    if evidence_lines:
        print("  DEBUG evidence capture:")
        for line in evidence_lines:
            print(f"    {line}")
    else:
        print("  DEBUG evidence capture: no loader/page-walk lines observed")

    entry_ran = "OK\n" in serial_output or "OK\r\n" in serial_output
    if entry_ran:
        print("  ✅ Rust ELF entry output (OK; second PT_LOAD bytes survived)")
    else:
        print("  ❌ Rust ELF entry output missing")
        all_ok = False

    new_pages = {int(v, 16): int(p, 16) for v, p in re.findall(
        r"DBG ELF TRACK: new vaddr=0x([0-9a-fA-F]+) paddr=0x([0-9a-fA-F]+)",
        serial_output,
    )}
    reused_pages = {int(v, 16): int(p, 16) for v, p in re.findall(
        r"DBG ELF REUSE: vaddr=0x([0-9a-fA-F]+) paddr=0x([0-9a-fA-F]+)",
        serial_output,
    )}
    entry_page = 0x2000000
    if (new_pages.get(entry_page)
            and (entry_page not in reused_pages
                 or reused_pages[entry_page] == new_pages[entry_page])):
        detail = ("overlapping PT_LOAD reuse validated"
                  if entry_page in reused_pages
                  else "mapped once; no overlapping PT_LOAD")
        print(f"  ✅ Entry page provenance preserved ({detail})")
    else:
        print("  ❌ Entry page provenance evidence missing or frame changed")
        all_ok = False

    exception = "VIBIX: EXCEPTION:" in serial_output
    if exception:
        print("  ❌ Exception detected before scheduling evidence")
        all_ok = False
    else:
        print("  ✅ No exception detected")

    global_irq_observations = serial_output.count("IRQ frame:")
    print(f"  ◇ Global IRQ observations (informational only): {global_irq_observations}")
    interval_fixture_ok = _check_scheduler_interval_fixture()
    print(f"  {'✅' if interval_fixture_ok else '❌'} Exact disjoint scheduler interval fixture")
    all_ok &= interval_fixture_ok
    scheduler_ok = _check_rust_elf_scheduler_evidence(serial_output)
    return all_ok and not exception and scheduler_ok


def test_vibit_rust():
    """Run a bounded Rust ELF probe with readiness-aware accelerator retries."""
    print("🧪 Testing VIBIT with Rust ELF (multi-segment)...")
    if not test_probe_selection():
        return False
    with tempfile.NamedTemporaryFile(mode="w+", suffix=".serial", delete=False) as f:
        serial_path = f.name
    try:
        probe = _run_bounded_qemu(
            serial_path,
            _available_accelerators(),
            completion_markers=("OK\n", "OK\r\n"),
            completion_predicate=_scheduler_probe_complete,
            timeout=60,
        )
        serial_output = probe.transcript
        _print_selected_probe("Rust ELF probe", probe, include_transcript=True)
        if not _rust_probe_result_is_acceptable(probe):
            return False
        return _print_rust_elf_result(serial_output)
    finally:
        if os.path.exists(serial_path):
            os.unlink(serial_path)


def _synthetic_large_elf():
    """Build an uncommitted ET_EXEC with 257 mapped pages and an OK marker."""
    base = 0x02000000
    message = b"LARGE_ELF_OK\n"
    code = bytearray()
    code += b"\xb8\x01\x00\x00\x00"          # write
    code += b"\xbf\x01\x00\x00\x00"
    code += b"\x48\xbe" + struct.pack("<Q", base + 0x100)
    code += b"\xba" + struct.pack("<I", len(message))
    code += b"\x0f\x05"
    # Stay runnable after publishing the marker long enough for the scheduler
    # evidence threshold before exiting. VIBIT must then reap this child and
    # respawn the /bin/vish path before the bounded probe accepts a second marker.
    code += b"\xb9" + struct.pack("<I", 0x20000000)  # ecx = bounded spin count
    spin_start = len(code)
    code += b"\xff\xc9"                         # dec ecx
    code += b"\x0f\x85" + struct.pack("<i", spin_start - (len(code) + 6))
    code += b"\xb8\x00\x00\x00\x00"          # exit (VIBIX syscall 0)
    code += b"\xbf\x00\x00\x00\x00\x0f\x05"  # status 0, syscall
    segment = bytearray(0x1000)
    segment[:len(code)] = code
    segment[0x100:0x100 + len(message)] = message
    image = bytearray(0x1000) + segment
    ehdr = struct.pack(
        "<16sHHIQQQIHHHHHH", b"\x7fELF" + bytes([2, 1, 1]) + bytes(9),
        2, 0x3E, 1, base, 64, 0, 0, 64, 56, 1, 0, 0, 0,
    )
    phdr = struct.pack(
        "<IIQQQQQQ", 1, 5, 0x1000, base, base, 0x1000, 0x101000, 0x1000,
    )
    image[:64] = ehdr
    image[64:64 + 56] = phdr
    return bytes(image)


def _synthetic_large_elf_lifecycle_status(serial_output):
    """Return ordered first-marker, reap/respawn, and continuation evidence."""
    fixture_marker = "LARGE_ELF_OK\n"
    respawn_marker = "VIBIT: respawning shell..."
    first = serial_output.find(fixture_marker)
    if first < 0:
        return False, "missing first LARGE_ELF_OK marker"
    respawn = serial_output.find(respawn_marker, first + len(fixture_marker))
    if respawn < 0:
        return False, "missing VIBIT reaper/respawn marker after first LARGE_ELF_OK"
    second_marker = serial_output.find(fixture_marker, respawn + len(respawn_marker))
    if second_marker < 0:
        return False, "missing second LARGE_ELF_OK continuation marker after respawn"
    return True, "first marker → VIBIT reap/respawn → second marker"


def _synthetic_large_elf_lifecycle_complete(serial_output):
    """Completion predicate used by the bounded QEMU runner."""
    complete, _ = _synthetic_large_elf_lifecycle_status(serial_output)
    return complete


def _synthetic_large_elf_probe_complete(serial_output):
    """Wait for lifecycle completion plus the required scheduler evidence."""
    if not _synthetic_large_elf_lifecycle_complete(serial_output):
        return False
    with contextlib.redirect_stdout(io.StringIO()):
        return _check_rust_elf_scheduler_evidence(serial_output)


def test_vibit_rust_large():
    """Execute the generated 257-page ELF through VIBIT's /bin/vish exec."""
    print("🧪 Testing VIBIT with synthetic 257-page ELF...")
    with tempfile.NamedTemporaryFile(suffix=".serial", delete=False) as serial:
        serial_path = serial.name
    staging = tempfile.mkdtemp(prefix="vibix-large-elf-")
    archive_path = Path("userspace/initramfs.tar")
    backup_fd, backup_name = tempfile.mkstemp(
        prefix="vibix-initramfs-backup-", suffix=".tar"
    )
    os.close(backup_fd)
    archive_backup = Path(backup_name)
    original_archive_bytes = archive_path.read_bytes()
    result = False
    archive_restored = False
    shutil.copyfile(archive_path, archive_backup)
    try:
        import tarfile
        with tarfile.open(archive_path) as archive:
            archive.extractall(staging)
        (Path(staging) / "bin" / "vish").write_bytes(_synthetic_large_elf())
        with tarfile.open(archive_path, "w", format=tarfile.USTAR_FORMAT) as archive:
            for name in ("sbin/init", "bin/vish"):
                archive.add(str(Path(staging) / name), arcname=name, recursive=False)

        # The archive is embedded by include_bytes! in the Rust kernel.  Build
        # after staging it, while preventing the normal userspace recipe from
        # replacing the fixture with the sibling NASM vish binary.
        build = subprocess.run(
            ["make", "DEBUG=1", "INIT=vibit", "VIBIX_STAGED_INITRAMFS=1", "vibix.elf"],
            cwd=Path(__file__).resolve().parent,
            capture_output=True,
            text=True,
            timeout=180,
        )
        if build.returncode != 0:
            print("  ❌ Synthetic ELF kernel build failed")
            print(build.stdout[-4000:])
            print(build.stderr[-4000:])
        else:
            marker = "LARGE_ELF_OK\n"
            probe = _run_bounded_qemu(
                serial_path,
                _available_accelerators(),
                attempts=1,
                completion_markers=(marker,),
                completion_predicate=_synthetic_large_elf_probe_complete,
                timeout=60,
            )
            output = probe.transcript
            probe_admitted = _rust_probe_result_is_acceptable(probe)
            print(
                f"  {'✅' if probe_admitted else '❌'} Rust probe admission "
                f"(selected status={probe.classification})"
            )
            fixture = _synthetic_large_elf()
            fixture_valid = (
                len(fixture) > 0x1000
                and struct.unpack_from("<Q", fixture, 24)[0] == 0x02000000
                and struct.unpack_from("<Q", fixture, 64 + 40)[0] == 0x101000
            )
            arena_ok = "ELF: provenance arena >256 pages and cleanup OK." in output
            interval_fixture_ok = _check_scheduler_interval_fixture()
            vish_handoff = (
                # DEBUG syscall/serial output can split the VIBIT string between
                # "spawning" and "shell"; the exec trace is authoritative.
                "VIBIT: spawning" in output
                and "DBG EXEC: path=/bin/vish" in output
            )
            reaper_loop = "VIBIT: reaper loop" in output
            lifecycle_ok, lifecycle_detail = _synthetic_large_elf_lifecycle_status(output)
            scheduler_ok = _check_rust_elf_scheduler_evidence(output)
            no_exception = "VIBIX: EXCEPTION:" not in output
            no_panic = "VIBIX: PANIC:" not in output
            bounded_completion = lifecycle_ok
            print(f"  {'✅' if fixture_valid else '❌'} Synthetic 257-page ELF generated at test time")
            print(f"  {'✅' if arena_ok else '❌'} Lower-level provenance capacity and cleanup")
            print(f"  {'✅' if vish_handoff else '❌'} VIBIT fork/exec handoff to /bin/vish")
            print(f"  {'✅' if reaper_loop else '❌'} VIBIT reaper loop observed")
            print(f"  {'✅' if marker in output else '❌'} First synthetic ELF completion marker (LARGE_ELF_OK)")
            print(f"  {'✅' if lifecycle_ok else '❌'} Clean exit, child reaping, respawn, and continuation ({lifecycle_detail})")
            print(f"  {'✅' if interval_fixture_ok else '❌'} Exact disjoint scheduler interval fixture")
            print(f"  {'✅' if scheduler_ok else '❌'} Correlated scheduler evidence ({RUST_SCHED_REQUIRED_ROUND_TRIPS} round trips)")
            print(f"  {'✅' if no_exception else '❌'} No exception detected")
            print(f"  {'✅' if no_panic else '❌'} No kernel panic detected")
            print(f"  {'✅' if bounded_completion else '❌'} Bounded lifecycle completion before QEMU cutoff")
            result = (
                probe_admitted
                and
                fixture_valid
                and arena_ok
                and vish_handoff
                and reaper_loop
                and lifecycle_ok
                and interval_fixture_ok
                and scheduler_ok
                and no_exception
                and no_panic
                and bounded_completion
            )
            if not result:
                print("  Serial transcript:", repr(output))
    finally:
        if os.path.exists(serial_path):
            os.unlink(serial_path)
        shutil.rmtree(staging, ignore_errors=True)
        try:
            shutil.copyfile(archive_backup, archive_path)
            archive_restored = archive_path.read_bytes() == original_archive_bytes
        except OSError as exc:
            print(f"  ❌ Original initramfs archive restoration failed: {exc}")
        finally:
            archive_backup.unlink(missing_ok=True)
        # Do not leave a kernel embedding the temporary fixture behind.  The
        # next normal make will rebuild from the restored archive.
        for name in (
            "kernel64_entry.o", "interrupts.o", "syscall_entry.o", "context_switch.o",
            "kernel64.elf", "kernel64.bin", "boot.o", "vibix.elf",
        ):
            (Path(__file__).resolve().parent / name).unlink(missing_ok=True)
    if archive_restored:
        print("  ✅ Original initramfs archive restored byte-for-byte")
    else:
        print("  ❌ Original initramfs archive was not restored byte-for-byte")
    return result and archive_restored


def test_tty_sigint():
    """Verify the DEBUG-only deterministic TTY Ctrl-C fixture."""
    print("🧪 Testing deterministic TTY Ctrl-C ownership fixture...")
    with tempfile.NamedTemporaryFile(suffix=".serial", delete=False) as serial:
        serial_path = serial.name
    try:
        probe = _run_bounded_qemu(serial_path, _available_accelerators(), attempts=1)
        serial_output = probe.transcript
        checks = {
            "TTY TEST: no-reader Ctrl-C PASS": "Ctrl-C with no waiting reader",
            "TTY TEST: missing-owner Ctrl-C PASS": "stale/missing waiting PID",
            "TTY TEST: nonblocked-owner Ctrl-C PASS": "live non-blocked waiting PID",
            "TTY TEST: blocked-owner Ctrl-C PASS": "blocked foreground reader",
            "TTY TEST: all Ctrl-C cases PASS": "all deterministic TTY cases",
        }
        ok = True
        for marker, label in checks.items():
            if marker in serial_output:
                print(f"  ✅ {label}")
            else:
                print(f"  ❌ {label} (missing: {marker!r})")
                ok = False
        for marker, label in (
            ("VIBIX: PANIC:", "no kernel panic"),
            ("VIBIX: EXCEPTION:", "no kernel exception"),
        ):
            if marker not in serial_output:
                print(f"  ✅ {label}")
            else:
                print(f"  ❌ {label} (found: {marker!r})")
                ok = False
        if not ok:
            print("  Serial transcript:", repr(serial_output))
        return ok
    finally:
        if os.path.exists(serial_path):
            os.unlink(serial_path)


def test_exec_argv_env_runtime():
    """Exercise flat and ELF exec argv/env delivery through the production syscall."""
    debug_runtime = os.environ.get("VIBIX_EXEC_ABI_DEBUG_TEST") == "1"
    print("Testing real flat and ELF exec argv/env startup ABI...")
    try:
        test_exec_argv_vector_boundary_fixture_address()
        test_exec_argv_env_debug_evidence()
    except AssertionError as error:
        print(f"FAIL ABI runtime transcript guard: {error}")
        return False
    print("PASS vector-boundary fixture keeps argv0 mapped")
    if not _test_owned_process_group_cleanup():
        print("FAIL owned build process-group descendants terminated before cleanup")
        return False
    print("PASS owned build process-group descendants terminated before cleanup")
    repo = Path(__file__).resolve().parent
    archive_path = repo / "userspace/initramfs.tar"
    serial_path = None
    staging = None
    original_archive = None
    archive_snapshot_ready = False
    archive_restored = False
    result = False
    try:
        original_archive = archive_path.read_bytes() if archive_path.exists() else None
        archive_snapshot_ready = True
        clean = _run_owned_process_group(["make", "clean"], cwd=repo, timeout=120)
        if clean.returncode != 0:
            print("VIBIX clean failed before runtime ABI test")
            print(clean.stdout[-2000:])
            print(clean.stderr[-2000:])
            return False
        with tempfile.NamedTemporaryFile(suffix=".serial", delete=False) as serial:
            serial_path = serial.name
        staging = Path(tempfile.mkdtemp(prefix="vibix-exec-argv-env-"))
        init_source = staging / "init.asm"
        probe_source = staging / "argv_probe.asm"
        elf_probe_source = staging / "argv_elf_probe.asm"
        elf_probe_object = staging / "argv_elf_probe.o"
        elf_linker_script = staging / "argv_elf_probe.ld"
        init_bin = staging / "init"
        probe_bin = staging / "argv-probe"
        elf_probe_bin = staging / "argv-elf-probe"
        init_asm = """bits 64
default rel
start:
    mov rax, 9
    lea rdi, [rel target_path]
    lea rsi, [rel bad_argv]
    xor rdx, rdx
    syscall
    cmp rax, -14
    jne bad_errno
    mov rax, 9
    lea rdi, [rel target_path]
    lea rsi, [rsp-8]
    lea rcx, [rel argv0]
    mov [rsi], rcx
    xor rdx, rdx
    syscall
    cmp rax, -14
    jne bad_vector_boundary
    mov byte [rsp-1], 'x'
    lea rsi, [rsp-16]
    lea rcx, [rsp-1]
    mov [rsi], rcx
    xor rdx, rdx
    mov rax, 9
    lea rdi, [rel target_path]
    syscall
    cmp rax, -14
    jne bad_string_boundary
    mov rax, 9
    lea rdi, [rel target_path]
    lea rsi, [rel too_many_argv]
    xor rdx, rdx
    syscall
    cmp rax, -7
    jne bad_e2big
    mov rax, 9
    lea rdi, [rel target_path]
    lea rsi, [rel too_long_argv]
    xor rdx, rdx
    syscall
    cmp rax, -7
    jne bad_bytes_limit
    mov rax, 9
    lea rdi, [rel target_path]
    lea rsi, [rel argv]
    lea rdx, [rel envp]
    syscall
    jmp exec_returned
bad_errno:
    mov rax, 1
    mov rdi, 1
    lea rsi, [rel errno_failed]
    mov rdx, errno_failed_len
    syscall
    jmp terminate
bad_vector_boundary:
    mov rax, 1
    mov rdi, 1
    lea rsi, [rel vector_boundary_failed]
    mov rdx, vector_boundary_failed_len
    syscall
    jmp terminate
bad_string_boundary:
    mov rax, 1
    mov rdi, 1
    lea rsi, [rel string_boundary_failed]
    mov rdx, string_boundary_failed_len
    syscall
    jmp terminate
bad_e2big:
    mov rax, 1
    mov rdi, 1
    lea rsi, [rel e2big_failed]
    mov rdx, e2big_failed_len
    syscall
    jmp terminate
bad_bytes_limit:
    mov rax, 1
    mov rdi, 1
    lea rsi, [rel byte_limit_failed]
    mov rdx, byte_limit_failed_len
    syscall
    jmp terminate
exec_returned:
    mov rax, 1
    mov rdi, 1
    lea rsi, [rel returned_failed]
    mov rdx, returned_failed_len
    syscall
terminate:
    xor rdi, rdi
    mov rax, 0
    syscall
target_path: db '/bin/argv-probe', 0
bad_argv: dq 0x9000, 0
argv: dq argv0+0x2000000, arg1+0x2000000, 0
envp: dq env0+0x2000000, 0
too_many_argv: times 33 dq argv0+0x2000000
too_long_argv: dq too_long_string+0x2000000, 0
too_long_string: times 2048 db 'A'
    db 0
argv0: db '/bin/argv-probe', 0
arg1: db 'alpha', 0
env0: db 'ABI=ok', 0
errno_failed: db 'EXEC ABI RUNTIME FAIL: expected EFAULT', 10
errno_failed_len equ $-errno_failed
vector_boundary_failed: db 'EXEC ABI RUNTIME FAIL: argv vector boundary was not EFAULT', 10
vector_boundary_failed_len equ $-vector_boundary_failed
string_boundary_failed: db 'EXEC ABI RUNTIME FAIL: argv string boundary was not EFAULT', 10
string_boundary_failed_len equ $-string_boundary_failed
e2big_failed: db 'EXEC ABI RUNTIME FAIL: 33 argv entries were not E2BIG', 10
e2big_failed_len equ $-e2big_failed
byte_limit_failed: db 'EXEC ABI RUNTIME FAIL: oversized argv string was not E2BIG', 10
byte_limit_failed_len equ $-byte_limit_failed
returned_failed: db 'EXEC ABI RUNTIME FAIL: exec returned unexpectedly', 10
returned_failed_len equ $-returned_failed
"""
        if debug_runtime:
            debug_exec_prefix = """    mov rax, 9
    lea rdi, [rel fail_image_path]
    lea rsi, [rel fail_argv]
    lea rdx, [rel fail_envp]
    syscall
    cmp rax, -12
    jne image_failure_bad
    mov rax, 1
    mov rdi, 1
    lea rsi, [rel image_failure_preserved]
    mov rdx, image_failure_preserved_len
    syscall
    mov rax, 9
    lea rdi, [rel target_path]
    lea rsi, [rel argv]
    lea rdx, [rel envp]
    syscall
    jmp exec_returned
image_failure_bad:
    mov rax, 1
    mov rdi, 1
    lea rsi, [rel image_failure_failed]
    mov rdx, image_failure_failed_len
    syscall
    jmp terminate
"""
            init_asm = init_asm.replace(
                "start:\n    mov rax, 9",
                "start:\n" + debug_exec_prefix + "    mov rax, 9",
                1,
            )
            init_asm = init_asm.replace(
                "target_path: db '/bin/argv-probe', 0",
                "fail_image_path: db '/bin/argv-fail-image', 0\n"
                "target_path: db '/bin/argv-probe', 0\n"
                "fail_argv: dq fail_argv0+0x2000000, fail_argv1+0x2000000, 0\n"
                "fail_envp: dq fail_env0+0x2000000, 0\n"
                "fail_argv0: db '/bin/argv-elf-probe', 0\n"
                "fail_argv1: db 'alpha', 0\n"
                "fail_env0: db 'ELF=ok', 0",
                1,
            )
            init_asm = init_asm.replace(
                "returned_failed_len equ $-returned_failed",
                "returned_failed_len equ $-returned_failed\n"
                "image_failure_preserved: db 'EXEC ABI IMAGE FAILURE PRESERVED', 10\n"
                "image_failure_preserved_len equ $-image_failure_preserved\n"
                "image_failure_failed: db 'EXEC ABI IMAGE FAILURE FAIL', 10\n"
                "image_failure_failed_len equ $-image_failure_failed",
                1,
            )
        init_source.write_text(init_asm)
        probe_source.write_text("""bits 64
default rel
start:
    mov r9d, 1
    test rsp, 0xf
    jne fail
    inc r9d
    cmp rdi, 32
    je exact_byte_limit
    cmp rdi, 2
    jne fail
    inc r9d
    lea rcx, [rsp+8]
    cmp rsi, rcx
    jne fail
    inc r9d
    lea rcx, [rsi+24]
    cmp rdx, rcx
    jne fail
    inc r9d
    cmp qword [rsp], 2
    jne fail
    inc r9d
    cmp qword [rsi+16], 0
    jne fail
    inc r9d
    cmp qword [rdx+8], 0
    jne fail
    inc r9d
    mov r8, [rsi]
    cmp dword [r8], 0x6e69622f
    jne fail
    cmp dword [r8+4], 0x6772612f
    jne fail
    cmp word [r8+8], 0x2d76
    jne fail
    cmp dword [r8+10], 0x626f7270
    jne fail
    cmp word [r8+14], 0x0065
    jne fail
    inc r9d
    mov r8, [rsi+8]
    cmp dword [r8], 0x68706c61
    jne fail
    cmp word [r8+4], 0x0061
    jne fail
    inc r9d
    mov r8, [rdx]
    cmp dword [r8], 0x3d494241
    jne fail
    cmp word [r8+4], 0x6b6f
    jne fail
    cmp byte [r8+6], 0
    jne fail
    mov rax, 9
    lea rdi, [rel self_path]
    lea rsi, [rel exact_limit_argv]
    xor rdx, rdx
    syscall
    jmp exact_exec_returned
exact_byte_limit:
    cmp qword [rsp], 32
    jne exact_layout_fail
    lea rcx, [rsp+8]
    cmp rsi, rcx
    jne exact_layout_fail
    lea rcx, [rsi+264]
    cmp rdx, rcx
    jne exact_layout_fail
    cmp qword [rsi+256], 0
    jne exact_layout_fail
    cmp qword [rdx], 0
    jne exact_layout_fail
    mov r8, [rsi+248]
    mov ecx, 1984
check_exact_bytes:
    cmp byte [r8+rcx], 'A'
    jne exact_data_fail
    dec ecx
    jns check_exact_bytes
    cmp byte [r8+1985], 0
    jne exact_data_fail
report_pass:
    mov rax, 1
    mov rdi, 1
    lea rsi, [rel pass]
    mov rdx, pass_len
    syscall
    mov rax, 9
    lea rdi, [rel elf_path]
    lea rsi, [rel elf_argv]
    lea rdx, [rel elf_envp]
    syscall
    jmp exact_exec_returned
fail:
    mov rax, 1
    mov rdi, 1
    lea rsi, [rel failed]
    mov rdx, failed_len
    syscall
    mov rdi, 1
    jmp exit
exact_layout_fail:
    mov rax, 1
    mov rdi, 1
    lea rsi, [rel exact_layout_failed]
    mov rdx, exact_layout_failed_len
    syscall
    mov rdi, 1
    jmp exit
exact_data_fail:
    mov rax, 1
    mov rdi, 1
    lea rsi, [rel exact_data_failed]
    mov rdx, exact_data_failed_len
    syscall
    mov rdi, 1
    jmp exit
exact_exec_returned:
    mov rax, 1
    mov rdi, 1
    lea rsi, [rel exact_exec_failed]
    mov rdx, exact_exec_failed_len
    syscall
    mov rdi, 1
exit:
    mov rax, 0
    syscall
exact_limit_argv: times 31 dq short_string+0x2000000
    dq exact_limit_string+0x2000000, 0
self_path: db '/bin/argv-probe', 0
elf_path: db '/bin/argv-elf-probe', 0
elf_argv: dq elf_argv0+0x2000000, elf_argv1+0x2000000, 0
elf_envp: dq elf_env+0x2000000, 0
elf_argv0: db '/bin/argv-elf-probe', 0
elf_argv1: db 'alpha', 0
elf_env: db 'ELF=ok', 0
short_string: db 'x', 0
exact_limit_string: times 1985 db 'A'
    db 0
pass: db 'EXEC ABI FLAT PASS', 10
pass_len equ $-pass
failed: db 'EXEC ABI RUNTIME FAIL: entry validation', 10
failed_len equ $-failed
exact_layout_failed: db 'EXEC ABI RUNTIME FAIL: exact-limit vector layout', 10
exact_layout_failed_len equ $-exact_layout_failed
exact_data_failed: db 'EXEC ABI RUNTIME FAIL: exact-limit string bytes', 10
exact_data_failed_len equ $-exact_data_failed
exact_exec_failed: db 'EXEC ABI RUNTIME FAIL: exact-limit exec rejected', 10
exact_exec_failed_len equ $-exact_exec_failed
""")
        elf_probe_source.write_text("""bits 64
default rel
global _start
section .text
_start:
    test rsp, 0xf
    jne fail
    cmp rdi, 2
    jne fail
    lea rcx, [rsp+8]
    cmp rsi, rcx
    jne fail
    lea rcx, [rsi+24]
    cmp rdx, rcx
    jne fail
    cmp qword [rsp], 2
    jne fail
    cmp qword [rsi+16], 0
    jne fail
    cmp qword [rdx+8], 0
    jne fail
    mov r8, [rsi]
    mov rax, 0x6772612f6e69622f
    cmp qword [r8], rax
    jne fail
    mov rax, 0x72702d666c652d76
    cmp qword [r8+8], rax
    jne fail
    cmp dword [r8+16], 0x0065626f
    jne fail
    mov r8, [rsi+8]
    cmp dword [r8], 0x68706c61
    jne fail
    cmp word [r8+4], 0x0061
    jne fail
    mov r8, [rdx]
    cmp dword [r8], 0x3d464c45
    jne fail
    cmp word [r8+4], 0x6b6f
    jne fail
    cmp byte [r8+6], 0
    jne fail
    mov rax, 1
    mov rdi, 1
    lea rsi, [rel pass]
    mov rdx, pass_len
    syscall
    xor rdi, rdi
    mov rax, 0
    syscall
fail:
    mov rax, 1
    mov rdi, 1
    lea rsi, [rel failed]
    mov rdx, failed_len
    syscall
    mov rdi, 1
    mov rax, 0
    syscall
pass: db 'EXEC ABI ELF PASS', 10
pass_len equ $-pass
failed: db 'EXEC ABI ELF FAIL', 10
failed_len equ $-failed
""")
        elf_linker_script.write_text("""ENTRY(_start)
PHDRS { text PT_LOAD FLAGS(5); }
SECTIONS {
    . = 0x02000000;
    .text : ALIGN(0x1000) { *(.text*) *(.rodata*) } :text
}
""")
        for source, binary in ((init_source, init_bin), (probe_source, probe_bin)):
            assembled = subprocess.run(
                ["nasm", "-f", "bin", str(source), "-o", str(binary)],
                capture_output=True, text=True, timeout=10,
            )
            if assembled.returncode != 0:
                print(f"NASM fixture build failed: {assembled.stderr.strip()}")
                return False

        assembled_elf = subprocess.run(
            ["nasm", "-f", "elf64", str(elf_probe_source), "-o", str(elf_probe_object)],
            capture_output=True, text=True, timeout=10,
        )
        if assembled_elf.returncode != 0:
            print(f"ELF NASM fixture build failed: {assembled_elf.stderr.strip()}")
            return False
        linked_elf = subprocess.run(
            ["ld", "-T", str(elf_linker_script), "-o", str(elf_probe_bin), str(elf_probe_object)],
            capture_output=True, text=True, timeout=10,
        )
        if linked_elf.returncode != 0:
            print(f"ELF linker fixture build failed: {linked_elf.stderr.strip()}")
            return False

        (staging / "root/sbin").mkdir(parents=True)
        (staging / "root/bin").mkdir(parents=True)
        shutil.copyfile(init_bin, staging / "root/sbin/init")
        shutil.copyfile(probe_bin, staging / "root/bin/argv-probe")
        shutil.copyfile(elf_probe_bin, staging / "root/bin/argv-elf-probe")
        shutil.copyfile(elf_probe_bin, staging / "root/bin/argv-fail-image")
        import tarfile
        with tarfile.open(archive_path, "w", format=tarfile.USTAR_FORMAT) as archive:
            for name in (
                "sbin/init", "bin/argv-probe", "bin/argv-elf-probe",
                "bin/argv-fail-image",
            ):
                archive.add(staging / "root" / name, arcname=name, recursive=False)

        build_command = ["make"]
        if debug_runtime:
            build_command.append("DEBUG=1")
        build_command.extend(("-j1", "VIBIX_STAGED_INITRAMFS=1", "vibix.elf"))
        build = _run_owned_process_group(
            build_command,
            cwd=repo, timeout=180,
        )
        if build.returncode != 0:
            print("Runtime ABI kernel build failed")
            print(build.stdout[-2500:])
            print(build.stderr[-2500:])
            return False

        if debug_runtime:
            probe = _run_bounded_qemu(
                serial_path, _available_accelerators(), attempts=1,
                completion_predicate=_exec_argv_env_debug_completion, timeout=45,
            )
        else:
            probe = _run_bounded_qemu(
                serial_path, _available_accelerators(), attempts=1,
                completion_markers=("EXEC ABI ELF PASS\n", "EXEC ABI ELF FAIL\n", "EXEC ABI RUNTIME FAIL"), timeout=45,
            )
        output = probe.transcript
        flat_pass = "EXEC ABI FLAT PASS" in output
        elf_pass = "EXEC ABI ELF PASS" in output
        pass_seen = flat_pass and elf_pass
        fail_seen = "EXEC ABI RUNTIME FAIL" in output or "EXEC ABI ELF FAIL" in output or "exec returned unexpectedly" in output
        exception = "VIBIX: EXCEPTION:" in output or "VIBIX: PANIC:" in output
        print(f"{'PASS' if flat_pass and not fail_seen else 'FAIL'} flat exec EFAULT/page boundaries, E2BIG limits, exact 32-vector/2048-byte acceptance, and entry ABI")
        print(f"{'PASS' if elf_pass and not fail_seen else 'FAIL'} ELF exec entry registers, stack vectors, and argument/environment strings")
        if debug_runtime:
            scheduled_return, failure_preserved = _exec_argv_env_debug_evidence(output)
            print(f"{'PASS' if failure_preserved else 'FAIL'} failed exec after image construction leaves caller image usable")
            print(f"{'PASS' if scheduled_return else 'FAIL'} forced scheduled-frame exec startup registers")
            fail_seen |= "EXEC ABI IMAGE FAILURE FAIL" in output
            pass_seen &= failure_preserved and scheduled_return
        print(f"{'PASS' if not exception else 'FAIL'} no kernel panic or exception")
        if not pass_seen or fail_seen or exception:
            print(f"Serial transcript tail ({len(output)} chars):", repr(output[-4000:]))
        result = pass_seen and not fail_seen and not exception
    finally:
        if serial_path and os.path.exists(serial_path):
            os.unlink(serial_path)
        if staging is not None:
            shutil.rmtree(staging, ignore_errors=True)
        if archive_snapshot_ready:
            try:
                if original_archive is None:
                    archive_path.unlink(missing_ok=True)
                    archive_restored = not archive_path.exists()
                else:
                    archive_path.write_bytes(original_archive)
                    archive_restored = archive_path.read_bytes() == original_archive
            except OSError as exc:
                print(f"Original initramfs archive restoration failed: {exc}")
        for name in (
            "kernel64_entry.o", "interrupts.o", "syscall_entry.o", "context_switch.o",
            "kernel64.elf", "kernel64.bin", "boot.o", "vibix.elf",
        ):
            (repo / name).unlink(missing_ok=True)
    print(f"{'PASS' if archive_restored else 'FAIL'} original initramfs archive restored byte-for-byte")
    return result and archive_restored


def test_elf_rollback():
    """Verify DEBUG-only ELF rollback and allocator reclamation cases."""
    entry_asm = (Path(__file__).resolve().parent / "kernel/syscall_entry.asm").read_text()
    scheduled_frame = entry_asm.split(".normal_push:", 1)[1].split(".after_push:", 1)[0]
    for register in ("rdi", "rsi", "rdx"):
        instruction = f"push qword [rel syscall_state.entry_{register}]"
        if instruction not in scheduled_frame:
            print(f"  ❌ scheduled exec frame does not restore {register.upper()}")
            return False
    print("🧪 Testing deterministic ELF partial-load rollback fixture...")
    with tempfile.NamedTemporaryFile(suffix=".serial", delete=False) as serial:
        serial_path = serial.name
    try:
        probe = _run_bounded_qemu(
            serial_path,
            _available_accelerators(),
            attempts=1,
            completion_markers=("ELF TEST: rollback final PASS\n",),
        )
        serial_output = probe.transcript
        checks = {
            "ELF TEST: truncated-later-segment rollback PASS": "truncated later PT_LOAD",
            "ELF TEST: pmm-exhaustion rollback PASS": "PMM exhaustion",
            "ELF TEST: metadata-exhaustion rollback PASS": "metadata exhaustion",
            "ELF TEST: rollback suite PASS": "complete rollback suite",
            "ELF TEST: stack-allocation rollback PASS": "stack allocation rollback",
            "ELF TEST: rollback final PASS": "final rollback marker",
            "PAGING TEST: fork leaf isolation PASS": "fork leaf frame isolation",
            "PAGING TEST: scratch success cleanup PASS": "scratch branch success cleanup",
            "PAGING TEST: fork leaf failure PASS": "fork clone failure cleanup",
            "FLAT TEST: allocation rollback PASS": "flat allocation rollback",
            "FLAT TEST: oversize rejection PASS": "flat oversize rejection",
            "FLAT TEST: old-root no-overwrite PASS": "flat old-root isolation",
            "FLAT TEST: small-binary zero-tail/no-stale copy PASS": "small-binary zero-tail and stale-PMM isolation",
            "EXEC ABI TEST: input vectors PASS": "bounded argv/env input validation",
            "EXEC ABI TEST: flat/ELF stack vectors PASS": "flat and ELF argc/argv/envp stack layout and empty vectors",
        }
        ok = True
        for marker, label in checks.items():
            if marker in serial_output:
                print(f"  ✅ {label}")
            else:
                print(f"  ❌ {label} (missing: {marker!r})")
                ok = False
        for marker, label in (
            ("VIBIX: PANIC:", "no kernel panic"),
            ("VIBIX: EXCEPTION:", "no kernel exception"),
        ):
            if marker not in serial_output:
                print(f"  ✅ {label}")
            else:
                print(f"  ❌ {label} (found: {marker!r})")
                ok = False
        if not ok:
            print("  Serial transcript:", repr(serial_output))
        return ok
    finally:
        if os.path.exists(serial_path):
            os.unlink(serial_path)
