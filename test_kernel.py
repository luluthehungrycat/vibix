#!/usr/bin/env python3
"""Test: run VIBIX kernel in QEMU and check for expected output."""
import subprocess
import sys
import os
import tempfile

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
}

OPTIONAL = {
    "VIBIX: Multiboot memory map:":       "Multiboot memory map",
    "VIBIX: EXCEPTION:":                  "Exception handler (future)",
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


if __name__ == "__main__":
    sys.exit(0 if test_kernel_alive() else 1)
