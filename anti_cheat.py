#!/usr/bin/env python3
import os
import sys
import difflib
from pathlib import Path

# Known kernel code patterns to avoid
FORBIDDEN_PATTERNS = [
    "Linux",
    "BSD",
    "GPL",
    "include/uapi",
    "include/linux",
    "include/asm",
    "SYSCALL_DEFINE",
    "__user",
    "current->",
    "list_head",
    "rcu_",
    "smp_",
    "EXPORT_SYMBOL",
    "module_init",
    "MODULE_LICENSE",
]

# Directories to scan
SCAN_DIRS = ["src", "include", "kernel", "."]

def check_file(filepath):
    with open(filepath, "r", encoding="utf-8", errors="ignore") as f:
        content = f.read()
        lines = content.splitlines()

    issues = []
    for i, line in enumerate(lines, 1):
        for pattern in FORBIDDEN_PATTERNS:
            if pattern in line:
                issues.append(f"  Line {i}: {line.strip()[:80]}")

    return issues

def main():
    issues_found = False
    for scan_dir in SCAN_DIRS:
        if not os.path.exists(scan_dir):
            continue
        for root, _, files in os.walk(scan_dir):
            for file in files:
                if file.endswith((".c", ".h", ".asm", ".S")):
                    filepath = os.path.join(root, file)
                    issues = check_file(filepath)
                    if issues:
                        print(f"\n⚠️  Potential contamination in {filepath}:")
                        for issue in issues:
                            print(issue)
                        issues_found = True
    if not issues_found:
        print("✅ No forbidden patterns found.")
    else:
        print("\n❌ Forbidden patterns detected. Review the above files.")
        sys.exit(1)

if __name__ == "__main__":
    main()
