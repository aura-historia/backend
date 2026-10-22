import contextlib
import io
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import textwrap
import unittest
from unittest.mock import patch


SCRIPT = pathlib.Path(__file__).resolve().parents[1] / "collect-coverage.sh"
sys.path.insert(0, str(SCRIPT.parent))


class ExistingCoverageCommand(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = pathlib.Path(self.temporary.name)
        self.profiles = self.root / "profile directory"
        self.profiles.mkdir()
        self.binaries = self.root / "binary directory"
        self.report = self.root / "reports" / "lane.lcov"
        self.record = self.root / "arguments.json"
        executable = self.root / "grcov"
        executable.write_text('''#!/usr/bin/env python3
import json,os,pathlib,sys
pathlib.Path(os.environ['FIXTURE_ARGUMENTS']).write_text(json.dumps(sys.argv[1:]))
pathlib.Path(os.environ['FIXTURE_ARGUMENTS']).with_suffix('.environment.json').write_text(json.dumps({'RAYON_NUM_THREADS': os.environ.get('RAYON_NUM_THREADS')}))
if os.environ.get('FIXTURE_EXIT'): raise SystemExit(int(os.environ['FIXTURE_EXIT']))
if not os.environ.get('FIXTURE_EMPTY'):
    pathlib.Path(sys.argv[sys.argv.index('-o')+1]).write_text('SF:src/example.rs\\nend_of_record\\n')
''')
        executable.chmod(0o755)
        self.environment = dict(os.environ, PATH=str(self.root) + os.pathsep + os.environ["PATH"],
                                FIXTURE_ARGUMENTS=str(self.record))

    def collect(self):
        return subprocess.run(["bash", str(SCRIPT), str(self.profiles), str(self.binaries), str(self.report)],
                              cwd=self.root, env=self.environment, text=True, capture_output=True)

    def test_export_keeps_existing_grcov_policy_and_handles_paths_with_spaces(self):
        (self.profiles / "case.profraw").write_bytes(b"profile")
        result = self.collect()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(self.record.read_text()), [str(self.profiles), "--threads", "1", "--binary-path", str(self.binaries),
            "-s", ".", "-t", "lcov", "--branch", "--ignore-not-existing", "--ignore", "/*",
            "--ignore", "*/target/*", "--ignore", "*/.cargo/*", "-o", str(self.report)])
        self.assertTrue(self.report.stat().st_size)

    def test_export_bounds_llvm_workers_independently_from_parser_threads_and_reports_resources(self):
        (self.profiles / 'case.profraw').write_bytes(b'profile')
        self.environment['RAYON_NUM_THREADS'] = '9'
        result = self.collect()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(self.record.with_suffix('.environment.json').read_text()), {'RAYON_NUM_THREADS': '1'})
        self.assertIn('COVERAGE_RESOURCES', result.stderr)
        self.assertIn('exit_code=0', result.stderr)

    def test_missing_profiles_fail_before_export(self):
        result = self.collect()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("No LLVM coverage profiles", result.stderr)
        self.assertFalse(self.record.exists())

    def test_exporter_failure_keeps_its_status(self):
        (self.profiles / "case.profraw").write_bytes(b"profile")
        self.environment["FIXTURE_EXIT"] = "37"
        self.assertEqual(self.collect().returncode, 37)

    def test_empty_export_is_rejected(self):
        (self.profiles / "case.profraw").write_bytes(b"profile")
        self.environment["FIXTURE_EMPTY"] = "1"
        self.assertNotEqual(self.collect().returncode, 0)

    def test_lane_and_legacy_configuration_reset_fresh_bounded_profile_namespaces(self):
        action = SCRIPT.parents[1] / '.github/actions/test-lane/action.yml'
        block = action.read_text().split('      run: |\n', 1)[1].split('\n    - name:', 1)[0]
        script = textwrap.dedent(block)
        profiles = []
        for lane, attempt in [('api', '1'), ('api', '2'), ('legacy-money', '1')]:
            with self.subTest(lane=lane, attempt=attempt):
                destination = self.root / (lane + '-' + attempt + '.env')
                temporary = self.root / 'runner temp'
                namespace = temporary / 'coverage' / lane
                stale = namespace / 'profiles/discovery/stale.profraw'
                stale.parent.mkdir(parents=True, exist_ok=True)
                stale.write_bytes(b'previous invocation')
                previous_report = namespace / (lane + '.lcov')
                previous_report.write_text('previous report')
                result = subprocess.run(['bash', '-euo', 'pipefail', '-c', script], env=dict(self.environment,
                    LANE_ID=lane, GITHUB_RUN_ID='123', GITHUB_RUN_ATTEMPT=attempt,
                    GITHUB_ENV=str(destination), RUNNER_TEMP=str(temporary), GITHUB_WORKSPACE=str(self.root)),
                    text=True, capture_output=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                emitted = dict(line.split('=', 1) for line in destination.read_text().splitlines())
                expected = str(namespace / 'profiles/discovery/%b-%4m.profraw')
                self.assertEqual(emitted['LLVM_PROFILE_FILE'], expected)
                self.assertFalse(stale.exists())
                self.assertFalse(previous_report.exists())
                profiles.append(expected)
        self.assertEqual(profiles[0], profiles[1])
        self.assertNotEqual(profiles[0], profiles[2])

    def test_fixture_snapshot_precedes_execution_upload_and_preserves_failure_diagnostics(self):
        action = (SCRIPT.parents[1] / '.github/actions/test-lane/action.yml').read_text()
        name = 'Record fixture image identities after execution'
        self.assertLess(action.index('Run owned targets and doctests'), action.index(name))
        self.assertLess(action.index(name), action.index('Preserve execution diagnostics before coverage export'))
        step = action.split('    - name: ' + name + '\n', 1)[1].split('\n    - name:', 1)[0]
        self.assertIn('      if: always()\n', step)
        self.assertIn('      continue-on-error: true\n', step)
        script = textwrap.dedent(step.split('      run: |\n', 1)[1])
        source = script.split("python3 - <<'PY'\n", 1)[1].rsplit('\nPY', 1)[0]
        identities = [{'reference': 'postgres:16-bookworm', 'identity': '[] sha256:local-image'},
                      {'reference': 'floci/floci:2.3.0', 'identity': None}]
        for lane, error in [('api', None), ('legacy-test-api', ValueError('Fixture image policy changed')),
                            ('crawler', FileNotFoundError('fixture source missing'))]:
            with self.subTest(lane=lane), patch.dict(os.environ, RUNNER_TEMP=str(self.root), LANE_ID=lane), patch(
                    'ci_benchmark.execution.images', return_value=identities, side_effect=error), contextlib.redirect_stdout(io.StringIO()):
                exec(compile(source, str(SCRIPT), 'exec'), {})
                record = json.loads((self.root / 'lane' / lane / 'fixture-images.json').read_text())
                self.assertEqual(record, {'images': identities if error is None else None,
                                          'diagnostic': None if error is None else f'{type(error).__name__}: {error}'})


if __name__ == "__main__":
    unittest.main()
