#!/usr/bin/env python3
"""Test: run VIBIX kernel in QEMU and check for expected output."""
import subprocess
import sys
import os
import tempfile

QEMU = "/usr/bin/qemu-system-x86_64"


def test_kernel_alive():
    print("🧪 Testing VIBIX kernel...")

    with tempfile.NamedTemporaryFile(mode="w+", suffix=".serial", delete=False) as f:
        serial_path = f.name

    try:
        result = subprocess.run(
            [
                QEMU,
                "-accel", "tcg",
                "-kernel", "vibix.elf",
                "-serial", f"file:{serial_path}",
                "-display", "none",
                "-m", "512M",
                "-no-reboot",
                "-no-shutdown",
            ],
            capture_output=True,
            text=True,
            timeout=10,
        )

        with open(serial_path, "r") as f:
            serial_output = f.read()

        if "VIBIX: Kernel alive!" in serial_output:
            print("✅ Test passed: Kernel printed alive message.")
            return True
        else:
            print("❌ Test failed: Kernel did not print alive message.")
            print("Serial output:", repr(serial_output))
            return False

    except subprocess.TimeoutExpired:
        # Timed out — the kernel should still have written to the serial file
        if os.path.exists(serial_path):
            with open(serial_path, "r") as f:
                serial_output = f.read()
            if "VIBIX: Kernel alive!" in serial_output:
                print("✅ Test passed (timeout): Kernel printed alive message.")
                return True
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
