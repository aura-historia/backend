import json
import os
import pathlib
import re
import subprocess
import sys
import tempfile
import textwrap
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]
ACTION = (ROOT / '.github/actions/test-lane/action.yml').read_text()
WORKFLOW = (ROOT / '.github/workflows/integrate.yml').read_text()


def action_step(name):
    return ACTION.split('    - name: ' + name + '\n', 1)[1].split('\n    - name:', 1)[0]


def run_step(name, environment):
    script = textwrap.dedent(action_step(name).split('      run: |\n', 1)[1])
    return subprocess.run(['bash', '-euo', 'pipefail', '-c', script], cwd=ROOT, env=environment,
                          text=True, capture_output=True)


class LaneCoverageMode(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = pathlib.Path(temporary.name)
        self.environment = dict(os.environ, LANE_ID='integration', COVERAGE_MODE='none',
            GITHUB_ENV=str(self.root / 'runner.env'), GITHUB_WORKSPACE=str(self.root),
            RUNNER_TEMP=str(self.root / 'runner'), CARGO_TARGET_DIR=str(self.root / 'target/no-coverage'))

    def test_none_clears_inherited_instrumentation_and_resets_only_its_target(self):
        target = pathlib.Path(self.environment['CARGO_TARGET_DIR'])
        target.mkdir(parents=True)
        (target / 'stale').touch()
        instrumented = self.root / 'target/coverage/retained'
        instrumented.parent.mkdir(parents=True)
        instrumented.touch()
        cleared = ('RUSTFLAGS', 'RUSTDOCFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'CARGO_ENCODED_RUSTDOCFLAGS',
                   'LLVM_PROFILE_FILE', 'AURA_CI_COVERAGE_DIR', 'AURA_CI_RUSTDOC')
        environment = dict(self.environment, **{key: 'inherited-instrumentation' for key in cleared},
                           RUSTDOC='/inherited/coverage-rustdoc.py')
        result = run_step('Configure test lane', environment)
        self.assertEqual(result.returncode, 0, result.stderr)
        emitted = dict(line.split('=', 1) for line in (self.root / 'runner.env').read_text().splitlines())
        effective = dict(environment, **emitted)
        self.assertEqual({key: effective[key] for key in cleared}, dict.fromkeys(cleared, ''))
        self.assertEqual(effective['RUSTDOC'], 'rustdoc')
        self.assertEqual(effective['RUSTUP_TOOLCHAIN'], 'nightly-2026-10-20')
        self.assertEqual((effective['CARGO_PROFILE_DEV_DEBUG'], effective['CARGO_PROFILE_TEST_DEBUG']), ('1', '1'))
        self.assertEqual(effective['CARGO_TARGET_DIR'], str(target))
        self.assertEqual(list(target.iterdir()), [])
        self.assertTrue(instrumented.is_file())
        self.assertFalse((self.root / 'runner/coverage').exists())
        self.assertIn('cache restore/save disabled', result.stdout)
        self.assertIn("      if: inputs.coverage == 'llvm'\n", action_step('Restore lane compilation cache'))
        for name in ('Export LLVM coverage', 'Upload coverage'):
            self.assertIn("      if: always() && inputs.coverage == 'llvm'\n", action_step(name))

    def test_unknown_mode_fails_before_resetting_outputs(self):
        target = pathlib.Path(self.environment['CARGO_TARGET_DIR'])
        target.mkdir(parents=True)
        marker = target / 'retained'
        marker.touch()
        for mode in ('', 'LLVM', 'disabled'):
            with self.subTest(mode=mode):
                result = run_step('Configure test lane', dict(self.environment, COVERAGE_MODE=mode))
                self.assertNotEqual(result.returncode, 0)
                self.assertTrue(marker.is_file())
                self.assertFalse((self.root / 'runner.env').exists())

    def test_preparation_passes_the_selected_coverage_policy_without_changing_the_guard(self):
        step = action_step('Prepare hosted runner disk capacity')
        self.assertIn('COVERAGE_MODE: ${{ inputs.coverage }}', step)
        self.assertIn('bash ci/prepare-runner-disk.sh "$COVERAGE_MODE"', step)
        for name in ('Build job supervisor', 'Run owned targets and doctests'):
            self.assertIn('python3 ci/guard-disk.py --target "$CARGO_TARGET_DIR"', action_step(name))

    def test_both_modes_keep_the_same_guarded_build_and_lane_commands(self):
        tools = self.root / 'tools'
        tools.mkdir()
        recorder = tools / 'python3'
        recorder.write_text(f'#!{sys.executable}\nimport json,os,pathlib,sys\n'
            "if sys.argv[1:] == ['-']: exec(compile(sys.stdin.read(), '<selection>', 'exec')); raise SystemExit\n"
            "pathlib.Path(os.environ['RECORDED_COMMAND']).write_text(json.dumps(sys.argv[1:]))\n"
            "raise SystemExit(int(os.environ.get('RECORDED_EXIT', '0')))\n")
        recorder.chmod(0o755)
        record = self.root / 'command.json'
        output = self.root / 'runner/lane/integration'
        output.mkdir(parents=True)
        build = ['cargo', 'build', '--locked', '--package', 'test-api', '--all-features', '--bin',
                 'aura-test-env', '--message-format=json-render-diagnostics']
        lane_commands = []
        for mode in ('none', 'llvm'):
            environment = dict(self.environment, PATH=str(tools) + os.pathsep + os.environ['PATH'],
                AURA_CI_COVERAGE_DIR='' if mode == 'none' else str(self.root / 'coverage'),
                RECORDED_COMMAND=str(record), LANES='INFRASTRUCTURE', LEGACY_PACKAGE='', PARITY_PACKAGES='',
                HARNESS='nextest', SLOTS='4', PROFILE='api', COVERAGE_MODE=mode)
            for name in ('Build job supervisor', 'Run owned targets and doctests'):
                record.unlink(missing_ok=True)
                result = run_step(name, environment)
                self.assertEqual(result.returncode, 0, result.stderr)
                if name == 'Build job supervisor' and mode == 'none':
                    self.assertFalse(record.exists())
                    self.assertIn('Lane discovery builds and validates', result.stdout)
                    continue
                command = json.loads(record.read_text())
                self.assertEqual(command[0], 'ci/guard-disk.py')
                self.assertEqual('--profiles' in command, mode == 'llvm')
                child = command[command.index('--') + 1:]
                if name == 'Build job supervisor':
                    if mode == 'llvm':
                        self.assertEqual(child[:2], ['python3', 'ci/capture-cargo.py'])
                        child = child[child.index('--') + 1:]
                    self.assertEqual(child, build)
                else:
                    lane_commands.append(child)
                    self.assertEqual(child[:4], ['python3', 'ci/run-test-lane.py', '--lane', 'INFRASTRUCTURE'])
                    self.assertEqual(child[child.index('--features') + 1],
                        'test-api/supervisor,test-api/s3,test-api/ses,test-api/aura-historia-api,test-api/api-gateway,test-api/eventbridge')
            failed = run_step('Build job supervisor', dict(environment, RECORDED_EXIT='37', LANES='API'))
            self.assertEqual(failed.returncode, 37, failed.stderr)
        self.assertEqual(lane_commands[0], lane_commands[1])

    def test_none_omits_build_only_for_native_infrastructure_selections(self):
        step = action_step('Build job supervisor')
        self.assertIn('from ci_inventory.discovery import discovery_builds_supervisor', step)
        for lanes in ('INFRASTRUCTURE', 'INFRASTRUCTURE REMAINING', 'API CRAWLER INFRASTRUCTURE REMAINING'):
            with self.subTest(lanes=lanes):
                result = run_step('Build job supervisor', dict(self.environment, LANES=lanes, LEGACY_PACKAGE=''))
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn('Lane discovery builds and validates', result.stdout)


class OrdinaryQualityGate(unittest.TestCase):
    def test_four_complete_lanes_keep_their_existing_configuration(self):
        parts = re.split(r'\n  ([a-z]+):\n', WORKFLOW.split('\njobs:', 1)[1])
        jobs = dict(zip(parts[1::2], parts[2::2]))
        self.assertEqual(set(jobs), {'lint', 'api', 'crawler', 'integration', 'remaining', 'quality'})
        for name, lane, profile in (('api', 'API', 'api'), ('crawler', 'CRAWLER', 'postgres'),
                ('integration', 'INFRASTRUCTURE', 'api'), ('remaining', 'REMAINING', 'none')):
            with self.subTest(lane=lane):
                inputs = dict(re.findall(r'^          (\w+): (\S+)$', jobs[name], re.MULTILINE))
                self.assertEqual(inputs, dict(id=name, lanes=lane, harness='nextest', slots='4',
                                              profile=profile, coverage='none'))
                self.assertIn('    runs-on: ubuntu-24.04\n', jobs[name])
        self.assertIn('uses: ./.github/actions/lint-workspace', jobs['lint'])
        self.assertNotIn('Sonar', WORKFLOW)
        self.assertIn("  coverage:\n    description:", ACTION)
        self.assertIn('    default: llvm\n', ACTION)

    def test_final_gate_rejects_any_missing_failed_cancelled_or_skipped_job(self):
        block = WORKFLOW.split('\n  quality:\n', 1)[1]
        self.assertIn('    needs: [lint, api, crawler, integration, remaining]\n', block)
        self.assertIn('    if: always()\n', block)
        script = textwrap.dedent(block.split("python3 - <<'PY'\n", 1)[1].split('\n          PY', 1)[0])
        successful = {name: {'result': 'success'} for name in ('lint', 'api', 'crawler', 'integration', 'remaining')}
        cases = [successful, {}, dict(successful, unexpected={'result': 'success'})]
        for name in successful:
            cases.append({key: value for key, value in successful.items() if key != name})
            cases.extend(dict(successful, **{name: {'result': result}}) for result in ('failure', 'cancelled', 'skipped'))
        for results in cases:
            with self.subTest(results=results):
                process = subprocess.run([sys.executable, '-c', script],
                    env=dict(os.environ, RESULTS=json.dumps(results)), capture_output=True)
                self.assertEqual(process.returncode == 0, results == successful)


if __name__ == '__main__':
    unittest.main()
