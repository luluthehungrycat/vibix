#!/usr/bin/env python3
"""Test: run VIBIX kernel in QEMU and check for expected output."""
import subprocess
import sys
import os
import tempfile
import selectors
import time
import socket

QEMU = "/usr/bin/qemu-system-x86_64"


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

        with open(serial_path, "r") as f:
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
            with open(serial_path, "r") as f:
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


def test_vibit():
    """Test the kernel with VIBIT init system (make INIT=vibit)."""
    print("🧪 Testing VIBIT init system...")

    with tempfile.NamedTemporaryFile(mode="w+", suffix=".serial", delete=False) as f:
        serial_path = f.name

    accel = "kvm"
    try:
        with open("/dev/kvm", "rb"):
            pass
    except (FileNotFoundError, PermissionError, OSError):
        accel = "tcg"

    try:
        subprocess.run(
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
    except subprocess.TimeoutExpired:
        pass  # Expected — shell blocks waiting for input

    with open(serial_path, "r") as f:
        serial_output = f.read()

    all_ok = True
    for marker, label in VIBIT_CHECKLIST.items():
        if marker in serial_output:
            print(f"  ✅ {label}")
        else:
            print(f"  ❌ {label} (missing: {marker!r})")
            all_ok = False

    os.unlink(serial_path)
    integration_ok = test_vibit_integration()
    return all_ok and integration_ok



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
    proc = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    connection = None
    output = bytearray()
    sent_input = False
    sent_exit = False
    deadline = time.monotonic() + 30
    try:
        while connection is None and time.monotonic() < deadline:
            try:
                connection = socket.create_connection(("127.0.0.1", port), timeout=0.2)
            except OSError:
                if proc.poll() is not None:
                    break
        if connection is None:
            print("  ❌ serial TCP connection was not established")
            return False
        connection.setblocking(False)
        selector = selectors.DefaultSelector()
        selector.register(connection, selectors.EVENT_READ)
        while time.monotonic() < deadline:
            events = selector.select(max(0, deadline - time.monotonic()))
            if not events:
                break
            for key, _ in events:
                chunk = key.fileobj.recv(4096)
                if not chunk:
                    break
                output.extend(chunk)
                text = output.decode("utf-8", errors="replace")
                if not sent_input and "vish$ " in text:
                    connection.sendall(b"help\n")
                    sent_input = True
                if sent_input and not sent_exit and "Built-in commands:" in text:
                    connection.sendall(b"exit\n")
                    sent_exit = True
                if sent_exit and text.count("vish$ ") >= 3 and "VIBIT: respawning shell..." in text:
                    deadline = time.monotonic()
                    break
        selector.close()
        text = output.decode("utf-8", errors="replace")
        checks = {
            "Built-in commands:": "deterministic vish command output",
            "VIBIT: reaped child": "VIBIT child reaping",
            "VIBIT: respawning shell...": "VIBIT shell continuation",
        }
        ok = sent_input and sent_exit and text.count("vish$ ") >= 3
        for marker, label in checks.items():
            if marker in text:
                print(f"  ✅ {label}")
            else:
                print(f"  ❌ {label} (missing: {marker!r})")
                ok = False
        if text.count("vish$ ") >= 3:
            print("  ✅ vish prompt returned after shell exit")
        else:
            print(f"  ❌ vish prompt repetition (observed {text.count('vish$ ')} prompt(s))")
        if not sent_input:
            print("  ❌ deterministic input was not sent (prompt missing)")
        if not sent_exit:
            print("  ❌ exit command was not sent (command output missing)")
        if not ok:
            print("  Serial transcript:", repr(text))
        return ok
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
