import importlib.util
import json
import pathlib
import sys
import tempfile
import unittest

CI = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(CI))
from ci_coverage import apply_branch_regions, combine, parse_lcov, render
from ci_benchmark.lcov import coverage_totals
from ci_inventory.phases import coverage_environment

SPEC = importlib.util.spec_from_file_location("export_coverage", CI / "export-coverage.py")
EXPORTER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(EXPORTER)


class NormalizedCoverage(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = pathlib.Path(self.temporary.name)
        (self.root / "src").mkdir()
        (self.root / "src/lib.rs").write_text("fn choose() {\n    if true { 1 } else { 2 };\n}\n")
        self.report = self.root / "coverage.lcov"

    def test_repeated_instantiations_and_incorrect_llvm_summaries_are_recomputed(self):
        text = "SF:src/lib.rs\nFN:1,choose_a\nFN:1,choose_b\nFNDA:1,choose_a\nFNDA:1,choose_b\nDA:1,2\nDA:2,2\nDA:3,0\nBRDA:2,0,0,1\nBRDA:2,0,1,0\nFNF:1\nFNH:1\nLF:3\nLH:1\nBRF:2\nBRH:0\nend_of_record\n"
        sources = parse_lcov(text, self.root)
        data = {"data": [{"functions": [], "files": [{"filename": str(self.root / "src/lib.rs"), "branches": [
            [2, 8, 2, 12, 1, 0, 0, 0, 4], [2, 8, 2, 12, 0, 1, 0, 0, 4]]}]}]}
        apply_branch_regions(sources, data, self.root)
        self.report.write_text(render(sources))
        totals = coverage_totals(self.report)["identity_union"]
        self.assertEqual((totals["lines_found"], totals["lines_hit"]), (3, 2))
        self.assertEqual((totals["branches_found"], totals["branches_hit"]), (2, 2))
        self.assertEqual(totals["functions_found"], 2)
        self.assertIn("BRDA:2,0,17,1", self.report.read_text())

    def test_source_union_keeps_unexecuted_branch_zero_and_one_record(self):
        first = parse_lcov("SF:src/lib.rs\nDA:2,1\nBRDA:2,8,0,1\nBRDA:2,8,1,0\nend_of_record\n", self.root)
        second = parse_lcov(f"SF:{self.root}/src/lib.rs\nDA:2,0\nBRDA:2,8,0,0\nBRDA:2,8,1,0\nend_of_record\n", self.root)
        combine(first, second)
        self.report.write_text(render(first))
        self.assertEqual(self.report.read_text().count("SF:"), 1)
        self.assertIn("BRDA:2,8,1,0", self.report.read_text())
        self.assertEqual(coverage_totals(self.report)["identity_union"]["branches_hit"], 1)

    def test_ambiguous_branch_spans_fail_instead_of_merging_different_conditions(self):
        sources = parse_lcov("SF:src/lib.rs\nDA:2,1\nend_of_record\n", self.root)
        data = {"data": [{"functions": [], "files": [{"filename": "src/lib.rs", "branches": [
            [2, 8, 2, 12, 1, 0, 0, 0, 4], [2, 8, 2, 15, 0, 1, 0, 0, 4]]}]}]}
        with self.assertRaisesRegex(ValueError, "Ambiguous"):
            apply_branch_regions(sources, data, self.root)

    def test_duplicate_branch_origins_within_one_function_fail_closed(self):
        sources = parse_lcov("SF:src/lib.rs\nDA:2,1\nend_of_record\n", self.root)
        function = {"filenames": ["src/lib.rs"], "branches": [
            [2, 8, 2, 12, 1, 0, 0, 0, 4], [2, 8, 2, 12, 0, 1, 0, 0, 4]]}
        with self.assertRaisesRegex(ValueError, "within a function"):
            apply_branch_regions(sources, {"data": [{"functions": [function], "files": []}]}, self.root)

    def test_external_sources_are_omitted_but_missing_or_invalid_repository_sources_fail(self):
        text = "SF:/rustc/library/std.rs\nDA:999999,1\nend_of_record\nSF:src/lib.rs\nDA:1,1\nend_of_record\n"
        self.assertEqual(set(parse_lcov(text, self.root)), {"src/lib.rs"})
        for content in ("", "SF:src/missing.rs\nend_of_record\n", "SF:src/lib.rs\nDA:4,1\nend_of_record\n",
                        "SF:src/lib.rs\nDA:1,1\n", "SF:src/lib.rs\nDA:0,1\nend_of_record\n"):
            with self.subTest(content=content), self.assertRaises(ValueError):
                parse_lcov(content, self.root)


class CompiledCoverageObjects(unittest.TestCase):
    def test_objects_follow_actual_metadata_and_missing_executable_fails(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            unit, child, supervisor = [root / name for name in ("build-out-test", "child-probe", "supervisor")]
            for path in (unit, child, supervisor):
                path.write_bytes(b"\x7fELFfixture")
            doctest = root / "doctests" / "crate" / "generated-name"
            doctest.parent.mkdir(parents=True)
            doctest.write_bytes(b"\x7fELFfixture")
            doctest.chmod(0o755)
            nextest = root / "nextest.json"
            nextest.write_text(json.dumps({"rust-suites": {"unit": {"binary-path": str(unit)}}, "rust-build-meta": {
                "target-directory": str(root), "non-test-binaries": {"pkg": [{"path": child.name}]}}}))
            cargo = root / "cargo.jsonl"
            cargo.write_text(json.dumps({"reason": "compiler-artifact", "executable": str(supervisor)}) + "\n")
            self.assertEqual(EXPORTER.executable_objects(nextest, [cargo], root / "doctests"),
                             sorted([unit, child, supervisor, doctest]))
            profiles = root / "profiles"
            (profiles / "discovery").mkdir(parents=True)
            (profiles / "discovery/list.profraw").write_bytes(b"listing is not execution")
            with self.assertRaisesRegex(ValueError, "execution coverage profiles"):
                EXPORTER.export(profiles, nextest, [cargo], root / "doctests", root / "out.lcov", root)
            (profiles / "empty.profraw").touch()
            with self.assertRaisesRegex(ValueError, "execution coverage profiles"):
                EXPORTER.export(profiles, nextest, [cargo], root / "doctests", root / "out.lcov", root)
            self.assertFalse((root / "out.lcov").exists())
            child.unlink()
            with self.assertRaisesRegex(ValueError, "executable is missing"):
                EXPORTER.executable_objects(nextest, [cargo], root / "doctests")

    def test_phase_profiles_preserve_credentials_and_subprocess_inheritance(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = {"AURA_CI_COVERAGE_DIR": temporary, "AWS_ACCESS_KEY_ID": "test", "RUSTFLAGS": "-C instrument-coverage"}
            managed = coverage_environment(base, "managed")
            doctests = coverage_environment(managed, "doctests")
            self.assertNotEqual(managed["LLVM_PROFILE_FILE"], doctests["LLVM_PROFILE_FILE"])
            self.assertEqual(managed["AWS_ACCESS_KEY_ID"], "test")
            self.assertEqual(managed["RUSTFLAGS"], doctests["RUSTFLAGS"])
            self.assertTrue(pathlib.Path(doctests["LLVM_PROFILE_FILE"]).parent.is_dir())
            self.assertEqual(coverage_environment({"LLVM_PROFILE_FILE": "old-%p.profraw"}, "managed"),
                             {"LLVM_PROFILE_FILE": "old-%p.profraw"})


if __name__ == "__main__":
    unittest.main()
