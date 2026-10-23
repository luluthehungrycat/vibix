#!/usr/bin/env python3
"""Screen VIBIX sources for known foreign-kernel signatures and code clones.

This is a review aid, not a proof of originality. Known-signature rules can
identify recognizable APIs and licensing markers. Optional reference roots
enable token-normalized similarity checks against locally supplied kernel
sources, for example ``python3 anti_cheat.py --reference-root /src/linux``.
Without a reference corpus, arbitrary copied or heavily rewritten code cannot
be detected by this tool and still requires human provenance review.
"""

from __future__ import annotations

import argparse
import os
import re
import sys
from pathlib import Path

FILE_EXTENSIONS = (".rs", ".c", ".h", ".asm", ".S", ".inc")
SCAN_DIRS = (".",)
EXCLUDED_DIRS = {
    ".git", ".hg", ".svn", ".codex", ".opencode", "target", "build", "dist",
    "__pycache__", ".pytest_cache", ".ruff_cache", ".mypy_cache",
}

# SPDX and license notices warrant human review, but their presence alone does
# not establish that executable source was copied.
LICENSE_PATTERNS = (
    re.compile(r"GNU\s+General\s+Public\s+License", re.IGNORECASE),
    re.compile(r"SPDX-License-Identifier\s*:\s*GPL(?:-[0-9.]+)?", re.IGNORECASE),
    re.compile(r"\bGPL-[23]\.0(?:-(?:only|or-later))?\b", re.IGNORECASE),
)

# Distinctive upstream APIs and constructs. Match token-like identifiers and
# normalize whitespace/comments before applying these rules; generic project
# names such as "Linux" are deliberately not signatures.
CODE_SIGNATURES = (
    ("Linux syscall definition macro", re.compile(r"\bSYSCALL_DEFINE\s*\d*\b")),
    ("Linux user-pointer annotation", re.compile(r"\b__user\b")),
    ("Linux kernel-pointer annotation", re.compile(r"\b__kernel\b")),
    ("Linux current-task access", re.compile(r"\bcurrent\s*->")),
    ("Linux list_head structure", re.compile(r"\bstruct\s+list_head\b")),
    ("Linux list traversal macro", re.compile(r"\b(?:list_for_each|list_for_each_entry)\b")),
    ("Linux spinlock type", re.compile(r"\bspinlock_t\b")),
    ("Linux RCU head structure", re.compile(r"\bstruct\s+rcu_head\b")),
    ("Linux RCU read-side API", re.compile(r"\brcu_read_(?:lock|unlock)\b")),
    ("Linux exported-symbol macro", re.compile(r"\bEXPORT_SYMBOL(?:_GPL)?\b")),
    ("Linux module lifecycle macro", re.compile(r"\bMODULE_(?:LICENSE|AUTHOR|DESCRIPTION)\b")),
    ("Linux module lifecycle function", re.compile(r"\bmodule_(?:init|exit)\b")),
    ("Linux memory-barrier API", re.compile(r"\bsmp_(?:mb|wmb|rmb)\b")),
    ("BSD kernel allocator declaration", re.compile(r"\bMALLOC_DEFINE\s*\(")),
    ("BSD kernel-only compilation guard", re.compile(r"\b_KERNEL\b")),
    ("Linux kernel include path", re.compile(r"#\s*include\s*[<\"]\s*(?:linux|asm|uapi)/")),
    ("BSD kernel header", re.compile(r"#\s*include\s*[<\"]sys/(?:cdefs|param|kernel)\.h")),
    ("kernel UAPI include path", re.compile(r"\binclude\s*/\s*(?:uapi|linux|asm)\b")),
)

TOKEN_RE = re.compile(r"[A-Za-z_][A-Za-z_0-9]*|\d+|->|::|==|!=|<=|>=|&&|\|\||[^\s]")
SHINGLE_SIZE = 9
MIN_REFERENCE_TOKENS = 40
MIN_SHARED_SHINGLES = 12
MIN_CONTAINMENT = 0.75

# Each exception is constrained to one file suffix and one exact text fragment.
# It suppresses only known-signature findings on that line, never clone matches
# or licensing markers.
EXCEPTIONS = (
    ("vfs/mod.rs", "x86_64 Linux struct stat"),
    ("vibix_stat_chdir.inc", "Linux x86_64"),
    ("syscall.rs", "Follows Linux convention"),
    ("pipe.rs", "Linux default"),
    ("paging.rs", "PAGE_KERNEL"),
    ("fb.rs", "PAGE_KERNEL"),
    ("gdt.rs", "GDT_KERNEL"),
)


def is_exception(filepath: str, line: str) -> bool:
    normalized = filepath.replace("\\", "/")
    return any(normalized.endswith(suffix) and fragment in line for suffix, fragment in EXCEPTIONS)


def _blank_preserving_lines(value: str) -> str:
    return "".join("\n" if char == "\n" else " " for char in value)


def code_only(text: str, suffix: str = "") -> str:
    """Remove comments and string/character literals while preserving offsets."""
    if suffix == ".rs":
        text = re.sub(r"'(?:\\.|[^'\\])'", lambda m: _blank_preserving_lines(m.group()), text)
    out: list[str] = []
    i = 0
    state = "code"
    quote = ""
    while i < len(text):
        pair = text[i : i + 2]
        char = text[i]
        if state == "line_comment":
            if char == "\n":
                out.append("\n")
                state = "code"
            else:
                out.append(" ")
            i += 1
            continue
        if state == "block_comment":
            if pair == "*/":
                out.extend((" ", " "))
                i += 2
                state = "code"
            else:
                out.append("\n" if char == "\n" else " ")
                i += 1
            continue
        if state == "string":
            if char == "\\" and i + 1 < len(text):
                out.extend((" ", "\n" if text[i + 1] == "\n" else " "))
                i += 2
            elif char == quote:
                out.append(" ")
                i += 1
                state = "code"
            else:
                out.append("\n" if char == "\n" else " ")
                i += 1
            continue
        if pair == "//":
            out.extend((" ", " "))
            i += 2
            state = "line_comment"
        elif pair == "/*":
            out.extend((" ", " "))
            i += 2
            state = "block_comment"
        elif char == '"' or (suffix in (".asm", ".inc") and char == "'"):
            quote = char
            out.append(" ")
            i += 1
            state = "string"
        elif suffix in (".asm", ".inc") and char == ";":
            out.append(" ")
            i += 1
            state = "line_comment"
        else:
            out.append(char)
            i += 1
    return "".join(out)


def _read_source(filepath: Path) -> str:
    return filepath.read_text(encoding="utf-8", errors="strict")


def check_file(filepath: str) -> list[str]:
    """Return licensing and known-signature findings; unreadable files fail closed."""
    path = Path(filepath)
    try:
        text = _read_source(path)
    except (OSError, UnicodeError) as error:
        return [f"  L0: unable to scan file: {error}"]

    issues: list[str] = []
    lines = text.splitlines()
    for number, line in enumerate(lines, 1):
        if any(pattern.search(line) for pattern in LICENSE_PATTERNS):
            issues.append(f"  L{number}: license marker requires review: {line.strip()[:100]}")

    executable = code_only(text, path.suffix)
    for label, pattern in CODE_SIGNATURES:
        for match in pattern.finditer(executable):
            number = executable.count("\n", 0, match.start()) + 1
            source_line = lines[number - 1] if number <= len(lines) else ""
            if not is_exception(filepath, source_line):
                issues.append(f"  L{number}: {label}: {source_line.strip()[:100]}")
    return issues


def source_files(root: Path, scan_roots: list[Path] | tuple[Path, ...]) -> list[Path]:
    """Return unique source paths in deterministic order, excluding generated trees."""
    root = root.resolve()
    found: set[Path] = set()
    for scan_root in scan_roots:
        scan_root = Path(scan_root)
        start = scan_root if scan_root.is_absolute() else root / scan_root
        if not start.exists():
            continue
        if start.is_file():
            if start.suffix in FILE_EXTENSIONS and not start.is_symlink():
                found.add(start.resolve())
            continue
        def raise_walk_error(error: OSError) -> None:
            raise error

        for current, dirs, files in os.walk(start, followlinks=False, onerror=raise_walk_error):
            dirs[:] = sorted(
                name for name in dirs
                if name not in EXCLUDED_DIRS and not (Path(current) / name).is_symlink()
            )
            for filename in files:
                path = Path(current) / filename
                if path.suffix in FILE_EXTENSIONS and not path.is_symlink():
                    found.add(path.resolve())
    return sorted(found, key=lambda path: path.as_posix())


def code_tokens(filepath: Path) -> list[str]:
    text = _read_source(filepath)
    return TOKEN_RE.findall(code_only(text, filepath.suffix))


def _shingles(tokens: list[str]) -> set[tuple[str, ...]]:
    return {
        tuple(tokens[index : index + SHINGLE_SIZE])
        for index in range(max(0, len(tokens) - SHINGLE_SIZE + 1))
    }


def find_reference_matches(project_root: Path, reference_roots: list[Path]) -> list[str]:
    """Find files with substantial token-shingle overlap against supplied sources."""
    project_files = source_files(project_root, list(SCAN_DIRS))
    references: list[Path] = []
    for reference_root in reference_roots:
        reference_root = reference_root.resolve()
        if not reference_root.is_dir():
            raise ValueError(f"reference root is not a directory: {reference_root}")
        paths = source_files(reference_root, [Path(".")])
        if not paths:
            raise ValueError(f"reference root contains no supported source files: {reference_root}")
        references.extend(paths)
    project = [(path, code_tokens(path)) for path in project_files]
    reference = [(path, code_tokens(path)) for path in references]
    findings: list[str] = []
    for source_path, tokens in project:
        if len(tokens) < MIN_REFERENCE_TOKENS:
            continue
        source_shingles = _shingles(tokens)
        for reference_path, reference_tokens in reference:
            if source_path.resolve() == reference_path.resolve() or len(reference_tokens) < MIN_REFERENCE_TOKENS:
                continue
            reference_shingles = _shingles(reference_tokens)
            shared = len(source_shingles & reference_shingles)
            denominator = min(len(source_shingles), len(reference_shingles))
            if shared >= MIN_SHARED_SHINGLES and denominator and shared / denominator >= MIN_CONTAINMENT:
                findings.append(
                    f"{source_path}: token similarity with {reference_path} "
                    f"({shared} shared {SHINGLE_SIZE}-token shingles, "
                    f"{shared / denominator:.0%} containment)"
                )
    return sorted(set(findings))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path.cwd(), help="project root to scan")
    parser.add_argument(
        "--reference-root", type=Path, action="append", default=[],
        help="optional local kernel source tree to compare; may be repeated",
    )
    args = parser.parse_args(argv)
    root = args.root.resolve()
    try:
        files = source_files(root, list(SCAN_DIRS))
    except OSError as error:
        print(f"❌  Unable to traverse source tree: {error}", file=sys.stderr)
        return 1
    if not files:
        print(f"❌  No source files found under {root}")
        return 1

    findings: list[tuple[Path, list[str]]] = []
    for filepath in files:
        issues = check_file(str(filepath))
        if issues:
            findings.append((filepath, issues))

    try:
        reference_matches = find_reference_matches(root, args.reference_root) if args.reference_root else []
    except (OSError, UnicodeError, ValueError) as error:
        print(f"❌  Reference scan failed: {error}", file=sys.stderr)
        return 1
    for filepath, issues in findings:
        print(f"\n{'=' * 60}\n⚠️  {filepath}\n{'=' * 60}")
        print("\n".join(issues))
    if reference_matches:
        print("\nReference-source similarity findings:")
        for finding in reference_matches:
            print(f"  {finding}")

    if not args.reference_root:
        print("LIMITATION: no external reference tree configured; only known signatures and license markers were checked.")
    if findings or reference_matches:
        print("\n❌  Review flagged source before accepting it; findings are not automatic proof of copying.")
        return 1
    print(f"✅  No known signatures found in {len(files)} source files.")
    if args.reference_root:
        print("✅  No high-similarity source pairs found against the configured reference trees.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
