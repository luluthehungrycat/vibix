from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

import anti_cheat


class AntiCheatTests(unittest.TestCase):
    def test_generic_project_name_in_comment_is_not_a_code_signature(self):
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary) / "kernel.rs"
            source.write_text("// Linux syscall ABI compatibility note\nfn run() {}\n")

            self.assertEqual(anti_cheat.check_file(str(source)), [])

    def test_kernel_signature_matches_across_whitespace(self):
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary) / "kernel.c"
            source.write_text("struct\nlist_head { int next; };\n")

            issues = anti_cheat.check_file(str(source))

            self.assertTrue(any("list_head" in issue for issue in issues))

    def test_reference_comparison_detects_normalized_copied_code(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            project = root / "project"
            reference = root / "reference"
            project.mkdir()
            reference.mkdir()
            body = """
                fn walk_process_table(table: &mut [Process]) {
                    for entry in table.iter_mut() {
                        if entry.state == ProcessState::Ready {
                            entry.quantum = entry.quantum.saturating_sub(1);
                            if entry.quantum == 0 {
                                entry.state = ProcessState::Runnable;
                            }
                        }
                    }
                }
            """
            (project / "process.rs").write_text(body)
            (reference / "process.rs").write_text(body)

            findings = anti_cheat.find_reference_matches(project, [reference])

            self.assertTrue(findings)
            self.assertIn("process.rs", findings[0])

    def test_missing_reference_tree_is_not_reported_as_a_clean_comparison(self):
        with tempfile.TemporaryDirectory() as temporary:
            missing = Path(temporary) / "missing-kernel-tree"

            with self.assertRaisesRegex(ValueError, "reference root is not a directory"):
                anti_cheat.find_reference_matches(Path(temporary), [missing])

    def test_scan_roots_are_deduplicated_and_build_directories_skipped(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "kernel" / "core.rs"
            source.parent.mkdir()
            source.write_text("fn safe() {}\n")
            build_source = root / "target" / "generated.rs"
            build_source.parent.mkdir()
            build_source.write_text("SYSCALL_DEFINE0(fake)\n")

            paths = anti_cheat.source_files(root, [root, root / "kernel"])

            self.assertEqual(paths, [source])


if __name__ == "__main__":
    unittest.main()
