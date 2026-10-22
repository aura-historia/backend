import fnmatch
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import textwrap
import unittest

WORKFLOW = pathlib.Path(__file__).resolve().parents[2] / ".github/workflows/benchmark-lanes.yml"
SCRIPT = textwrap.dedent(WORKFLOW.read_text().split("python3 - <<'PYPLAN'\n", 1)[1].split("\n          PYPLAN", 1)[0])


class OptInLanePlan(unittest.TestCase):
    def plan(self, **values):
        environment = dict(os.environ, EVENT="workflow_dispatch", LABEL="", HEAD_SHA="", BASE_SHA="d" * 40,
            REVISION="a" * 40, PRIME_REVISION="",
            LAYOUT="four", SCENARIO="FULL", REPETITION="1", HARNESS="nextest", SLOTS="4",
            GITHUB_RUN_NUMBER="17", GITHUB_WORKFLOW_SHA="b" * 40)
        environment.update(values)
        with tempfile.TemporaryDirectory() as directory:
            output = pathlib.Path(directory) / "output"
            result = subprocess.run([sys.executable, "-c", SCRIPT], env=dict(environment, GITHUB_OUTPUT=str(output)),
                                    text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            return result, dict(line.split("=", 1) for line in output.read_text().splitlines()) if output.exists() else {}

    def test_every_layout_covers_each_lane_once(self):
        for layout, count in (("four", 4), ("three", 3), ("one", 1)):
            with self.subTest(layout=layout):
                result, outputs = self.plan(LAYOUT=layout)
                self.assertEqual(result.returncode, 0, result.stderr)
                rows = json.loads(outputs["matrix"])
                self.assertEqual(len(rows), count)
                self.assertEqual(sorted(lane for row in rows for lane in row["lanes"].split()),
                                 ["API", "CRAWLER", "INFRASTRUCTURE", "REMAINING"])
                self.assertEqual(outputs["revision"], "a" * 40)

    def test_label_bootstrap_uses_actual_head_and_explicit_screen_defaults(self):
        for layout in ("four", "three", "one"):
            with self.subTest(layout=layout):
                result, outputs = self.plan(EVENT="pull_request", LABEL="ci/lanes-" + layout,
                    HEAD_SHA="c" * 40, REVISION="", LAYOUT="", SCENARIO="", REPETITION="", HARNESS="", SLOTS="")
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual({name: outputs[name] for name in ("revision", "layout", "harness", "slots", "scenario", "repetition")},
                    dict(revision="c" * 40, layout=layout, harness="nextest", slots="4", scenario="FULL", repetition="17"))

    def test_invalid_source_or_configuration_fails_before_emitting_a_plan(self):
        for key, value in (("REVISION", "develop"), ("LAYOUT", "unknown"), ("SCENARIO", "unknown"),
                           ("REPETITION", "0"), ("HARNESS", "unknown"), ("SLOTS", "1024"), ("GITHUB_WORKFLOW_SHA", "unknown")):
            with self.subTest(key=key):
                result, outputs = self.plan(**{key: value})
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(outputs, {})

    def test_small_label_primes_the_actual_pr_base_without_filtering_any_lane(self):
        result, outputs = self.plan(EVENT='pull_request', LABEL='ci/lanes-three-leaf', HEAD_SHA='c'*40)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(outputs['prime_revision'], 'd'*40)
        self.assertEqual(outputs['revision'], 'c'*40)
        self.assertEqual(outputs['scenario'], 'LEAF')
        self.assertEqual(sorted(lane for row in json.loads(outputs['matrix']) for lane in row['lanes'].split()),
                         ['API', 'CRAWLER', 'INFRASTRUCTURE', 'REMAINING'])

    def test_small_dispatch_requires_a_distinct_immutable_priming_base(self):
        for prime in ('', 'develop', 'a'*40):
            with self.subTest(prime=prime):
                result, outputs = self.plan(SCENARIO='API', PRIME_REVISION=prime)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(outputs, {})

    def test_stock_harness_label_changes_only_harness_not_full_scope(self):
        result, outputs = self.plan(EVENT='pull_request', LABEL='ci/lanes-four-libtest', HEAD_SHA='c'*40)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((outputs['harness'], outputs['scenario'], outputs['prime_revision']), ('libtest', 'FULL', ''))
        self.assertEqual(len(json.loads(outputs['matrix'])), 4)

    def test_unselected_workflow_skips_quality_but_selected_failures_enter_its_gate(self):
        for workflow, job, prerequisite in ((WORKFLOW, 'quality', 'plan'),
                (WORKFLOW.with_name('compare-legacy-ci.yml'), 'quality', 'determine')):
            block = workflow.read_text().split('\n  ' + job + ':\n', 1)[1]
            guard = next(line.strip().removeprefix('if: ') for line in block.splitlines() if line.strip().startswith('if: '))
            for result in ('skipped', 'success', 'failure', 'cancelled'):
                with self.subTest(workflow=workflow.name, result=result):
                    expression = guard.replace('always()', 'True').replace('&&', 'and').replace(
                        f'needs.{prerequisite}.result', repr(result))
                    self.assertEqual(eval(expression, {'__builtins__': {}}), result != 'skipped')
            marker = "python3 - <<'PY'\n"
            script = textwrap.dedent(block.split(marker, 1)[1].split('\n          PY', 1)[0])
            successful = {name: {'result': 'success'} for name in (prerequisite, 'lint', 'test')}
            cases = [successful, {}, dict(successful, unexpected={'result': 'success'})]
            for name in successful:
                cases.append({key: value for key, value in successful.items() if key != name})
                cases.extend(dict(successful, **{name: {'result': result}}) for result in ('failure', 'cancelled', 'skipped'))
            for results in cases:
                process = subprocess.run([sys.executable, '-c', script], env=dict(os.environ, RESULTS=json.dumps(results)),
                                         stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                self.assertEqual(process.returncode == 0, results == successful)

    def test_controls_choose_none_and_require_execution_artifacts_instead_of_coverage(self):
        for path in (WORKFLOW, WORKFLOW.with_name('compare-legacy-ci.yml')):
            source = path.read_text()
            self.assertIn('--input execution-artifacts --matrix "$MATRIX"', source)
            self.assertNotIn('coverage-artifacts', source)
            self.assertNotIn('merge-coverage.py', source)
            self.assertNotIn('SonarSource/', source)
            global_environment = source.split('\njobs:', 1)[0]
            self.assertIn('  CARGO_PROFILE_TEST_DEBUG: 0\n', global_environment)
            self.assertNotIn('CARGO_PROFILE_DEV_DEBUG:', global_environment)
            self.assertIn('bash ci/prepare-runner-disk.sh none', source)
        self.assertIn('--coverage none --cache', WORKFLOW.read_text())
        self.assertIn('          coverage: none\n', WORKFLOW.with_name('compare-legacy-ci.yml').read_text())

    def test_legacy_gate_downloads_execution_artifacts_without_final_diagnostic_duplicates(self):
        source = WORKFLOW.with_name('compare-legacy-ci.yml').read_text()
        pattern = next(line.strip().removeprefix('pattern: ') for line in source.splitlines()
                       if line.strip().startswith('pattern: ')).replace('${{ github.run_attempt }}', '2')
        artifacts = ['tests-legacy-money-execution-attempt-2', 'tests-legacy-money-attempt-2',
                     'tests-legacy-test-api-execution-attempt-2', 'tests-legacy-test-api-attempt-2',
                     'tests-legacy-money-execution-attempt-1', 'legacy-selection-attempt-2']
        self.assertEqual(fnmatch.filter(artifacts, pattern),
                         ['tests-legacy-money-execution-attempt-2', 'tests-legacy-test-api-execution-attempt-2'])


if __name__ == "__main__":
    unittest.main()
