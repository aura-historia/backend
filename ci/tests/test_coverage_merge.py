import importlib.util
import pathlib
import sys
import tempfile
import unittest

CI = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(CI))
SPEC = importlib.util.spec_from_file_location("merge_coverage", CI / "merge-coverage.py")
MERGER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MERGER)
LCOV = "SF:src/lib.rs\nDA:1,1\nFNF:0\nFNH:0\nLF:1\nLH:1\nBRF:0\nBRH:0\nend_of_record\n"


class CompleteLaneCoverage(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.directory = pathlib.Path(self.temporary.name) / "artifacts"
        self.output = pathlib.Path(self.temporary.name) / "coverage.lcov"
        self.lanes = ["api", "crawler", "integration", "remaining"]
        (self.output.parent / "src").mkdir()
        self.reports = {}
        for lane in self.lanes:
            report = self.directory / f"coverage-{lane}" / f"{lane}.lcov"
            report.parent.mkdir(parents=True)
            report.write_text(LCOV.replace("src/lib.rs", f"src/{lane}.rs"))
            self.reports[lane] = report
            (self.output.parent / "src" / f"{lane}.rs").write_text("fn example() {}\n")

    def test_all_valid_reports_are_sorted_and_normalized(self):
        MERGER.merge(self.directory, self.output, list(reversed(self.lanes)), self.output.parent)
        self.assertEqual([row for row in self.output.read_text().splitlines() if row.startswith("SF:")],
                         [f"SF:src/{lane}.rs" for lane in sorted(self.lanes)])

    def test_shared_sources_across_lanes_do_not_duplicate_found_lines_or_branches(self):
        (self.output.parent / "src/shared.rs").write_text("fn shared() {}\n")
        for index, report in enumerate(self.reports.values()):
            report.write_text("SF:src/shared.rs\nFN:1,shared\nFNDA:1,shared\nDA:1,1\n" +
                              f"BRDA:1,4,0,{index % 2}\nBRDA:1,4,1,{1 - index % 2}\n" +
                              "FNF:1\nFNH:1\nLF:1\nLH:1\nBRF:2\nBRH:1\nend_of_record\n")
        MERGER.merge(self.directory, self.output, self.lanes, self.output.parent)
        text = self.output.read_text()
        self.assertEqual(text.count("SF:"), 1)
        self.assertIn("DA:1,4\n", text)
        self.assertIn("BRDA:1,4,0,2\n", text)
        self.assertIn("BRDA:1,4,1,2\n", text)
        self.assertIn("LF:1\nLH:1\nBRF:2\nBRH:2\n", text)

    def test_missing_report_is_failure(self):
        self.reports["api"].unlink()
        with self.assertRaisesRegex(ValueError, "missing="):
            MERGER.merge(self.directory, self.output, self.lanes, self.output.parent)
        self.assertFalse(self.output.exists())

    def test_unexpected_or_duplicate_report_is_failure(self):
        for extra in ("coverage-unexpected/extra.lcov", "coverage-api/duplicate.lcov"):
            with self.subTest(extra=extra):
                path = self.directory / extra
                path.parent.mkdir(exist_ok=True)
                path.write_text(LCOV)
                with self.assertRaisesRegex(ValueError, "unexpected="):
                    MERGER.merge(self.directory, self.output, self.lanes, self.output.parent)
                path.unlink()

    def test_empty_truncated_or_inconsistent_report_fails_before_output_changes(self):
        self.output.write_text("previous")
        for content in ("", LCOV.removesuffix("end_of_record\n"), LCOV.replace("LH:1", "LH:0")):
            with self.subTest(content=content):
                self.reports["api"].write_text(content)
                with self.assertRaises(ValueError):
                    MERGER.merge(self.directory, self.output, self.lanes, self.output.parent)
                self.assertEqual(self.output.read_text(), "previous")

    def test_empty_duplicate_or_unknown_expected_lanes_fail(self):
        for lanes in ([], ["api", "api"], ["foreign"], ["legacy-../api"], ["legacy-"]):
            with self.subTest(lanes=lanes), self.assertRaisesRegex(ValueError, "distinct, known"):
                MERGER.merge(self.directory, self.output, lanes, self.output.parent)

    def test_opt_in_legacy_reports_use_the_same_exact_artifact_validation(self):
        directory = self.directory / 'legacy'
        report = directory / 'coverage-legacy-money/legacy-money.lcov'
        report.parent.mkdir(parents=True)
        report.write_text(LCOV)
        MERGER.merge(directory, self.output, ['legacy-money'])
        self.assertEqual(self.output.read_text(), LCOV)
        with self.assertRaisesRegex(ValueError, 'missing='):
            MERGER.merge(directory, self.output, ['legacy-money', 'legacy-api'])


if __name__ == "__main__":
    unittest.main()
