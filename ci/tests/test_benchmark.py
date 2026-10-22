import json
import pathlib
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import call, patch

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
from ci_benchmark.metrics import coverage_totals, observations
from ci_benchmark.execution import fixture_references, images, optional_command


class BenchmarkMetrics(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.path = pathlib.Path(self.temporary.name) / "evidence.txt"

    def test_concatenated_lcov_reports_sum_and_union_without_double_counting_lines(self):
        self.path.write_text("""SF:src/lib.rs
DA:1,0
DA:2,1
FN:1,example
FNDA:0,example
BRDA:2,0,0,-
LF:2
LH:1
FNF:1
FNH:0
BRF:1
BRH:0
end_of_record
SF:src/lib.rs
DA:1,2
DA:2,0
FN:1,example
FNDA:1,example
BRDA:2,0,0,1
LF:2
LH:1
FNF:1
FNH:1
BRF:1
BRH:1
end_of_record
""")
        totals = coverage_totals(self.path)
        self.assertEqual(totals["record_sums"]["lines_found"], 4)
        self.assertEqual(totals["identity_union"]["lines_found"], 2)
        self.assertEqual(totals["identity_union"]["lines_hit"], 2)
        self.assertEqual(totals["identity_union"]["branches_hit"], 1)

    def test_missing_coverage_records_fail_instead_of_claiming_zero_coverage(self):
        self.path.write_text("grcov failed\n")
        with self.assertRaisesRegex(ValueError, "no LCOV"):
            coverage_totals(self.path)

    def test_truncated_malformed_or_inconsistent_coverage_cannot_be_collected(self):
        valid = "SF:src/lib.rs\nDA:1,1\nLF:1\nLH:1\nFNF:0\nFNH:0\nBRF:0\nBRH:0\nend_of_record\n"
        invalid = (valid.replace("end_of_record\n", ""), valid.replace("LF:1\n", ""),
                   valid.replace("LF:1", "LF:2"), valid.replace("DA:1,1", "DA:1,-1"),
                   valid.replace("SF:src/lib.rs", "SF:"), valid.replace("DA:1,1", "DA:0,1"),
                   valid.replace("LF:1", "LF:1\nLF:1"), "SF:src/lib.rs\nend_of_record\n",
                   valid.replace("end_of_record", "SF:other.rs\nend_of_record"),
                   valid.replace("LF:1", "FN:1,missing-count\nLF:1"),
                   "DA:1,1\n" + valid)
        for raw in invalid:
            with self.subTest(raw=raw):
                self.path.write_text(raw)
                with self.assertRaises(ValueError):
                    coverage_totals(self.path)

    def test_emitted_functions_with_same_name_are_counted_before_identity_union(self):
        self.path.write_text("SF:src/lib.rs\nFN:1,same\nFN:2,same\nFNDA:0,same\nFNDA:1,same\n"
                             "LF:0\nLH:0\nFNF:2\nFNH:1\nBRF:0\nBRH:0\nend_of_record\n")
        totals = coverage_totals(self.path)
        self.assertEqual(totals["record_sums"]["functions_found"], 2)
        self.assertEqual(totals["identity_union"]["functions_found"], 1)
        self.assertEqual(totals["identity_union"]["functions_hit"], 1)

    def test_interleaved_harness_output_and_failed_body_preserve_partial_phase_records(self):
        self.path.write_text('''test sample ... AURA_CI_TIMING {"phase":"SETUP","owner":"sample","pid":1,"elapsed_seconds":0.25}
thread sample panicked
AURA_CI_TIMING {"phase":"BODY","owner":"sample","pid":1,"elapsed_seconds":0.1}
AURA_CI_TIMING {"phase":"TEARDOWN","owner":"sample","pid":1,"elapsed_seconds":0.01}
test result: FAILED. 1 passed; 1 failed; 2 ignored; 0 measured; 0 filtered out; finished in 1.0s
''')
        actual = observations(self.path)
        self.assertEqual(actual["tests"]["totals"]["failed"], 1)
        self.assertEqual(actual["fixture_timings"]["seconds"]["TEARDOWN"], [0.01])
        self.assertEqual(actual["fixture_timings"]["seconds"]["POSTGRES_STARTUP"], [])

    def test_context_lifecycle_observations_are_collected_and_unknown_phases_still_fail(self):
        phases = ("CONTEXT_ACQUIRE", "POSTGRES_SLOT_RESET", "CONTEXT_CLEANUP")
        self.path.write_text("".join("AURA_CI_TIMING " + json.dumps({
            "phase": phase, "owner": "isolated::sample", "pid": 1, "elapsed_seconds": 0.125,
        }) + "\n" for phase in phases))
        actual = observations(self.path)["fixture_timings"]
        self.assertEqual(actual["status"], "COLLECTED")
        self.assertEqual(len(actual["records"]), len(phases))
        for phase in phases:
            self.assertEqual(actual["seconds"][phase], [0.125])
        self.path.write_text('AURA_CI_TIMING {"phase":"UNKNOWN","elapsed_seconds":0.1}\n')
        with self.assertRaisesRegex(ValueError, "Malformed benchmark timing record"):
            observations(self.path)

    def test_no_phase_samples_are_not_reported_as_zero_duration_measurements(self):
        self.path.write_text("no structured observations\n")
        actual = observations(self.path)
        self.assertEqual(actual["fixture_timings"]["status"], "NOT_OBSERVED")
        self.assertEqual(actual["tests"]["status"], "UNAVAILABLE")
        self.assertIsNone(actual["tests"]["totals"])

    def test_recognized_timing_records_with_interleaved_harness_output_fail_closed(self):
        for raw in ('test sample ... AURA_CI_TIMING ok{\n"phase":"SETUP"}\n',
                    'AURA_CI_TIMING test should_handle_foreign_key_relations ... {ok"phase":"SETUP"}\n',
                    'AURA_CI_TIMING {"phase":"SETUP","elapsed_seconds":0.1ok}\n',
                    'AURA_CI_TIMING {"phase":"SETUP","elapsed_seconds":0.1}\nok\nAURA_CI_TIMING {\n',
                    'AURA_CI_TIMING {"phase":"SETUP"}\n'):
            with self.subTest(raw=raw):
                self.path.write_text(raw)
                with self.assertRaisesRegex(ValueError, "Malformed benchmark timing record on line"):
                    observations(self.path)

    def test_fixture_image_mapping_requires_review_when_engine_or_emulator_changes(self):
        root = pathlib.Path(self.temporary.name)
        source = root / "src/test-api/src"
        source.mkdir(parents=True)
        (source / "postgres.rs").write_text('const POSTGRES_IMAGE: &str = "postgres:16-bookworm";')
        (source / "aws.rs").write_text('const IMAGE_TAG: &str = "2.3.0";')
        (source / "opensearch.rs").write_text('.engine_version("OpenSearch_3.5")')
        self.assertIn("opensearchproject/opensearch:3.5.0", fixture_references(root))
        managed = source / "environment"
        managed.mkdir()
        references = ['postgres:16-bookworm', 'floci/floci:2.3.0', 'opensearchproject/opensearch:3.5.0']
        policy = managed / "services.rs"
        policy.write_text("\n".join('const ' + name + ': &str = \n    "' + reference + '@sha256:' + 'a' * 64 + '";'
                                    for name, reference in zip(("POSTGRES_IMAGE", "FLOCI_IMAGE", "OPENSEARCH_IMAGE"), references)))
        self.assertEqual(6, len(fixture_references(root)))
        policy.write_text('const POSTGRES_IMAGE: &str = "postgres:latest";')
        with self.assertRaisesRegex(ValueError, "immutable digests"):
            fixture_references(root)
        policy.unlink()
        (source / "opensearch.rs").write_text('.engine_version("OpenSearch_2.19")')
        with self.assertRaisesRegex(ValueError, "Unreviewed"):
            fixture_references(root)

    def test_fixture_inspection_is_bounded_and_unavailable_identity_remains_null(self):
        root = pathlib.Path(self.temporary.name)
        references = ['postgres:16-bookworm', 'floci/floci:2.3.0']
        identity = '["postgres@sha256:example"] sha256:local-image'
        with patch('ci_benchmark.execution.fixture_references', return_value=references), patch(
                'ci_benchmark.execution.subprocess.run', side_effect=[
                    subprocess.CompletedProcess([], 0, identity + '\n', ''),
                    subprocess.TimeoutExpired(['docker'], 5)]) as inspect:
            self.assertEqual(images(root), [{'reference': references[0], 'identity': identity},
                                           {'reference': references[1], 'identity': None}])
        self.assertEqual(inspect.call_args_list, [call(
            ['docker', 'image', 'inspect', '--format', '{{json .RepoDigests}} {{.Id}}', reference],
            cwd=root, env=None, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=5)
            for reference in references])

    def test_optional_image_inspection_preserves_missing_identity_on_command_failure(self):
        for outcome in (FileNotFoundError(), PermissionError(), subprocess.CompletedProcess([], 1, '', 'unavailable')):
            with self.subTest(outcome=outcome), patch('ci_benchmark.execution.subprocess.run') as inspect:
                if isinstance(outcome, OSError):
                    inspect.side_effect = outcome
                else:
                    inspect.return_value = outcome
                self.assertIsNone(optional_command(['docker', 'image', 'inspect', 'fixture'], self.path.parent, timeout=5))


if __name__ == "__main__":
    unittest.main()
