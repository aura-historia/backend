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
        self.reports = {}
        for lane in self.lanes:
            report = self.directory / f"coverage-{lane}" / f"{lane}.lcov"
            report.parent.mkdir(parents=True)
            report.write_text(LCOV.replace("src/lib.rs", f"src/{lane}.rs"))
            self.reports[lane] = report

    def test_all_valid_reports_keep_the_existing_concatenation_boundary(self):
        MERGER.merge(self.directory, self.output, list(reversed(self.lanes)))
        self.assertEqual(self.output.read_text(), "".join(report.read_text() for report in sorted(self.reports.values())))

    def test_missing_report_is_failure(self):
        self.reports["api"].unlink()
        with self.assertRaisesRegex(ValueError, "missing="):
            MERGER.merge(self.directory, self.output, self.lanes)
        self.assertFalse(self.output.exists())

    def test_unexpected_or_duplicate_report_is_failure(self):
        for extra in ("coverage-unexpected/extra.lcov", "coverage-api/duplicate.lcov"):
            with self.subTest(extra=extra):
                path = self.directory / extra
                path.parent.mkdir(exist_ok=True)
                path.write_text(LCOV)
                with self.assertRaisesRegex(ValueError, "unexpected="):
                    MERGER.merge(self.directory, self.output, self.lanes)
                path.unlink()

    def test_empty_truncated_or_inconsistent_report_fails_before_output_changes(self):
        self.output.write_text("previous")
        for content in ("", LCOV.removesuffix("end_of_record\n"), LCOV.replace("LH:1", "LH:0")):
            with self.subTest(content=content):
                self.reports["api"].write_text(content)
                with self.assertRaises(ValueError):
                    MERGER.merge(self.directory, self.output, self.lanes)
                self.assertEqual(self.output.read_text(), "previous")

    def test_empty_duplicate_or_unknown_expected_lanes_fail(self):
        for lanes in ([], ["api", "api"], ["foreign"], ["legacy-../api"], ["legacy-"]):
            with self.subTest(lanes=lanes), self.assertRaisesRegex(ValueError, "distinct, known"):
                MERGER.merge(self.directory, self.output, lanes)

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
