#!/usr/bin/env python3
"""
Anti-plagiarism scanner for the VIBIX kernel.

Scans source files for patterns that suggest copy-pasted code from
Linux, BSD, or other GPL-licensed kernels.  Uses simple substring
matching — not a substitute for human review, but catches obvious
copies.

AGENTS.md requires this to be run before committing Phase 2+ code:
  \"Anti-plagiarism checker scans for Linux/BSD patterns.\"
"""
import os
import sys

# ── Forbidden patterns: Linux/BSD kernel code signatures ──────────────────────
# These are identifiers, APIs, and comments that strongly suggest
# copy-pasted code from foreign kernels.  False positives are possible
# for generic terms (e.g., "Linux" in a comment mentioning the project).
FORBIDDEN_PATTERNS = [
    # Kernel project names in suspicious contexts
    "Linux",
    "FreeBSD",
    "NetBSD",
    "OpenBSD",
    # GPL licensing
    "GNU General Public License",
    "GPL-2.0",
    "GPL-3.0",
    "SPDX-License-Identifier: GPL",
    # Linux kernel headers / API
    "#include <linux/",
    "#include <asm/",
    "#include <uapi/",
    "include/uapi",
    "include/linux",
    "include/asm",
    "SYSCALL_DEFINE",
    "__user",
    "__kernel",
    "current->",
    # Linux kernel data structures
    "struct list_head",
    "LIST_HEAD",
    "list_for_each",
    "spinlock_t",
    "struct rcu_head",
    "rcu_read_lock",
    "rcu_read_unlock",
    # Linux kernel macros / exports
    "EXPORT_SYMBOL",
    "EXPORT_SYMBOL_GPL",
    "module_init",
    "module_exit",
    "MODULE_LICENSE",
    "MODULE_AUTHOR",
    "MODULE_DESCRIPTION",
    "smp_mb",
    "smp_wmb",
    "smp_rmb",
    # BSD kernel identifiers
    "sys/cdefs.h",
    "sys/param.h",
    "sys/kernel.h",
    "_KERNEL",
    "MALLOC_DEFINE",
]

# Short patterns that need extra context to avoid false positives.
# These are checked only if they appear outside comment lines.
FORBIDDEN_SHORT = [
    "smp_",
    "rcu_",
]

# Directories to scan (relative to project root)
SCAN_DIRS = [
    "kernel_rust/src",
    "kernel",
    ".",
]

FILE_EXTENSIONS = (".rs", ".c", ".h", ".asm", ".S", ".inc")

# ── False-positive exceptions ─────────────────────────────────────────────────
# Lines matching these patterns are suppressed from the report.
# Each entry is a (filename_suffix, pattern) tuple — the line is excluded
# if the file path ends with the suffix AND the line contains the pattern.
EXCEPTIONS = [
    # VIBIX targets the Linux x86_64 syscall ABI — mentioning "Linux" in
    # comments about struct layout or ABI compatibility is intentional.
    ("vfs/mod.rs", "x86_64 Linux struct stat"),
    ("vibix_stat_chdir.inc", "Linux x86_64"),
    ("syscall.rs", "Follows Linux convention"),
    ("pipe.rs", "Linux default"),
    # PAGE_KERNEL is a common paging constant name, not Linux-specific
    ("paging.rs", "PAGE_KERNEL"),
    ("fb.rs", "PAGE_KERNEL"),
    # GDT descriptor names are hardware-defined, not Linux-specific
    ("gdt.rs", "GDT_KERNEL"),
]


def is_comment_line(line: str) -> bool:
    """Heuristic: skip lines that are clearly comments."""
    stripped = line.strip()
    return stripped.startswith("//") or stripped.startswith(";") or \
           stripped.startswith("#") or stripped.startswith("/*") or \
           stripped.startswith("*")


def is_exception(filepath: str, line: str) -> bool:
    """Return True if this match should be suppressed."""
    for suffix, pat in EXCEPTIONS:
        if filepath.endswith(suffix) and pat in line:
            return True
    return False


def check_file(filepath: str) -> list:
    """Return list of (lineno, line_text) for matches."""
    try:
        with open(filepath, "r", encoding="utf-8", errors="replace") as f:
            lines = f.readlines()
    except Exception:
        return []

    issues = []
    for i, raw in enumerate(lines, 1):
        line = raw.rstrip("\n")
        if is_exception(filepath, line):
            continue
        for pattern in FORBIDDEN_PATTERNS:
            if pattern in line:
                issues.append(f"  L{i}: {line.strip()[:100]}")
        for pattern in FORBIDDEN_SHORT:
            if pattern in line and not is_comment_line(line):
                issues.append(f"  L{i}: {line.strip()[:100]}")
    return issues


def main() -> int:
    issues_found = False
    for scan_dir in SCAN_DIRS:
        if not os.path.isdir(scan_dir):
            print(f"[{scan_dir}] directory not found, skipping")
            continue
        for root, _, files in os.walk(scan_dir):
            for fname in files:
                if not fname.endswith(FILE_EXTENSIONS):
                    continue
                path = os.path.join(root, fname)
                issues = check_file(path)
                if issues:
                    print(f"\n{'='*60}")
                    print(f"⚠️  {path}")
                    print(f"{'='*60}")
                    for issue in issues:
                        print(issue)
                    issues_found = True

    if not issues_found:
        print("✅  No forbidden patterns detected.")
        return 0
    else:
        print(f"\n❌  Forbidden patterns detected in some files.")
        print("    Review flagged lines — if they are intentional references")
        print("    (e.g. mentioning Linux in documentation), add an exception.")
        return 1


if __name__ == "__main__":
    sys.exit(main())
