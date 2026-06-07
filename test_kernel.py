#!/usr/bin/env python3
import subprocess
import sys
import time

def test_kernel_alive():
    print("🧪 Testing VIBIX kernel...")
    try:
        # Run QEMU with a timeout of 10 seconds
        result = subprocess.run(
            ["qemu-system-x86_64", "-fda", "os.bin", "-serial", "stdio", "-display", "none", "-m", "512M", "-no-reboot", "-no-shutdown"],
            capture_output=True,
            text=True,
            timeout=10
        )
        # Check for the expected output
        if "VIBIX: Kernel alive!" in result.stdout:
            print("✅ Test passed: Kernel printed alive message.")
            return True
        else:
            print("❌ Test failed: Kernel did not print alive message.")
            print("Output:", result.stdout)
            return False
    except subprocess.TimeoutExpired:
        print("❌ Test failed: QEMU timed out (kernel likely hung).")
        return False
    except Exception as e:
        print(f"❌ Test failed with error: {e}")
        return False

if __name__ == "__main__":
    sys.exit(0 if test_kernel_alive() else 1)
