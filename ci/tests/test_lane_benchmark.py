import importlib.util
import json
import pathlib
import sys
import tempfile
import types
import unittest
from unittest.mock import call, patch

CI = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(CI))
from ci_benchmark import execution
SPEC = importlib.util.spec_from_file_location("lane_benchmark", CI / "bench-test-lane.py")
BENCHMARK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BENCHMARK)


class LaneMeasurementCommands(unittest.TestCase):
    def setUp(self):
        self.args = types.SimpleNamespace(lane=["INFRASTRUCTURE", "REMAINING"], harness="libtest", slots=2,
            profile="api", features=["test-api/s3,test-api/ses"], legacy_package=None, coverage="llvm")
        self.root, self.target, self.output = map(pathlib.Path, ["/checkout", "/target", "/evidence"])

    def test_compile_execution_and_coverage_share_one_target_and_feature_configuration(self):
        commands = BENCHMARK.commands(self.args, self.root, self.target, self.output)
        self.assertEqual([name for name, _ in commands], ["COMPILE_LINK", "SUPERVISOR_COMPILE", "TESTS_AND_FIXTURES", "COVERAGE"])
        compile_command, execute_command = commands[0][1], commands[2][1]
        expected = [*execute_command, "--list-only"]
        expected[expected.index("--output") + 1] = "/evidence/compile-discovery"
        self.assertEqual(compile_command, expected)
        self.assertIn("/target/debug/aura-test-env", execute_command)
        self.assertIn("/evidence/state", execute_command)
        self.assertIn("/evidence/discovery", execute_command)
        self.assertIn("test-api/s3,test-api/ses", execute_command)
        self.assertEqual(execute_command[2:6], ["--lane", "INFRASTRUCTURE", "--lane", "REMAINING"])
        self.assertEqual(commands[-1][1], [sys.executable, "/checkout/ci/export-coverage.py",
            "--profiles", "/evidence/profiles", "--nextest", "/evidence/discovery/nextest.json",
            "--cargo-artifact-manifest", "/evidence/discovery/coverage-artifacts.json",
            "--doctests", "/evidence/doctests", "--cargo-artifacts", "/evidence/compile-discovery/compile.jsonl",
            "--cargo-artifacts", "/evidence/supervisor.jsonl",
            "--output", "/evidence/coverage.lcov"])
        self.assertEqual(commands[1][1][:5], [sys.executable, "/checkout/ci/capture-cargo.py", "--output", "/evidence/supervisor.jsonl", "--"])
        self.assertIn("--message-format=json-render-diagnostics", commands[1][1])

    def test_measurements_pin_nightly_and_record_the_selected_compiler_environment(self):
        with patch.object(BENCHMARK.subprocess, "check_output", return_value="/nightly/rustdoc\n"):
            environment = BENCHMARK.measurement_environment(self.root, self.target, self.output, 'llvm')
        self.assertEqual(environment["RUSTUP_TOOLCHAIN"], "nightly-2026-10-20")
        self.assertEqual(environment["RUSTFLAGS"], "-C instrument-coverage -Zcoverage-options=branch")
        self.assertEqual(environment["RUSTDOCFLAGS"], environment["RUSTFLAGS"])
        self.assertNotIn('CARGO_ENCODED_RUSTFLAGS', environment)
        self.assertNotIn('CARGO_ENCODED_RUSTDOCFLAGS', environment)
        self.assertEqual(environment["AURA_CI_RUSTDOC"], "/nightly/rustdoc")
        self.assertEqual(environment["LLVM_PROFILE_FILE"], "/evidence/profiles/discovery/%b-%4m.profraw")
        self.assertEqual(environment["CARGO_PROFILE_TEST_DEBUG"], environment["CARGO_PROFILE_DEV_DEBUG"])
        with patch.object(execution, "optional_command", return_value="selected-toolchain") as command:
            record = execution.environment_metadata(self.root, environment)
        self.assertEqual(command.call_args_list[:2],
                         [call(["rustc", "-vV"], self.root, environment), call(["cargo", "--version"], self.root, environment)])
        self.assertEqual(record["build_environment"]["RUSTUP_TOOLCHAIN"], "nightly-2026-10-20")

    def test_uninstrumented_provenance_retains_cleared_encoded_flags_and_capture_environment(self):
        environment = dict.fromkeys(('RUSTFLAGS', 'RUSTDOCFLAGS', 'CARGO_ENCODED_RUSTFLAGS',
            'CARGO_ENCODED_RUSTDOCFLAGS', 'AURA_CI_RUSTDOC', 'LLVM_PROFILE_FILE', 'AURA_CI_COVERAGE_DIR'), '')
        environment.update(CARGO_TARGET_DIR='/target/no-coverage', RUSTDOC='rustdoc')
        with patch.object(execution, 'optional_command', return_value='selected-toolchain'):
            record = execution.environment_metadata(self.root, environment)
        self.assertEqual({key: record['build_environment'][key] for key in environment}, environment)

    def test_none_retains_driver_and_supervisor_build_without_capture_or_export(self):
        instrumented = BENCHMARK.commands(self.args, self.root, self.target, self.output)
        self.args.coverage = 'none'
        commands = BENCHMARK.commands(self.args, self.root, self.target, self.output)
        self.assertEqual([name for name, _ in commands], ['COMPILE_LINK', 'SUPERVISOR_COMPILE', 'TESTS_AND_FIXTURES'])
        self.assertEqual(commands[0], instrumented[0])
        self.assertEqual(commands[2], instrumented[2])
        self.assertEqual(commands[1][1], instrumented[1][1][5:])

    def test_none_overrides_inherited_instrumentation_without_resolving_coverage_rustdoc(self):
        cleared = ('RUSTFLAGS', 'RUSTDOCFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'CARGO_ENCODED_RUSTDOCFLAGS',
                   'AURA_CI_RUSTDOC', 'LLVM_PROFILE_FILE', 'AURA_CI_COVERAGE_DIR')
        with patch.dict(BENCHMARK.os.environ, dict.fromkeys(cleared, 'inherited')), patch.object(
                BENCHMARK.subprocess, 'check_output') as command:
            environment = BENCHMARK.measurement_environment(self.root, self.target, self.output, 'none')
        command.assert_not_called()
        self.assertEqual({key: environment[key] for key in cleared}, dict.fromkeys(cleared, ''))
        self.assertEqual(environment['RUSTDOC'], 'rustdoc')
        self.assertEqual(environment['CARGO_BUILD_JOBS'], '4')
        self.assertEqual((environment['CARGO_PROFILE_DEV_DEBUG'], environment['CARGO_PROFILE_TEST_DEBUG']), ('1', '1'))

    def test_none_keeps_cold_then_warm_target_and_records_absent_coverage(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            self.args.target_dir, self.args.coverage = root / 'target', 'none'
            for cache in ('COLD_TARGET', 'WARM_TARGET'):
                self.args.cache, self.args.output = cache, root / cache
                with self.subTest(cache=cache), patch.object(BENCHMARK, 'arguments', return_value=self.args), patch.object(
                        BENCHMARK.subprocess, 'check_output', side_effect=[str(root), 'a'*40, 'b'*40, '']), patch.object(
                        BENCHMARK, 'images', return_value=[]), patch.object(BENCHMARK, 'environment_metadata', return_value={}), patch.object(
                        BENCHMARK, 'measure', side_effect=lambda name, *_: dict(name=name, exit_code=0)) as measure, patch.object(
                        BENCHMARK, 'coverage_totals') as coverage, patch.object(BENCHMARK, 'observations', return_value={}):
                    self.assertEqual(BENCHMARK.main(), 0)
                    coverage.assert_not_called()
                    self.assertTrue(all('--profiles' not in call.args[1] for call in measure.call_args_list))
                report = json.loads((self.args.output / 'benchmark.json').read_text())
                self.assertEqual(report['status'], 'PASSED')
                self.assertEqual(report['coverage_mode'], 'none')
                self.assertIsNone(report['coverage'])
                self.assertFalse((self.args.output / 'profiles').exists())
                self.assertFalse((self.args.output / 'coverage.lcov').exists())
                (self.args.target_dir / 'compiled').touch()

    def test_no_service_lane_omits_supervisor_compilation(self):
        self.args.lane, self.args.profile = ["REMAINING"], "none"
        self.assertEqual([name for name, _ in BENCHMARK.commands(self.args, self.root, self.target, self.output)],
                         ["COMPILE_LINK", "TESTS_AND_FIXTURES", "COVERAGE"])

    def test_all_package_owners_are_retained_for_a_single_job(self):
        self.args.lane = ["API", "CRAWLER", "INFRASTRUCTURE", "REMAINING"]
        command = BENCHMARK.commands(self.args, self.root, self.target, self.output)[0][1]
        self.assertEqual([command[index + 1] for index, value in enumerate(command) if value == "--lane"], self.args.lane)

    def test_legacy_mode_keeps_package_features_and_builds_supervisor_only_for_actual_controls(self):
        self.args.lane, self.args.harness, self.args.profile, self.args.features = None, "libtest", "none", []
        for package, supervisor in (("money", False), ("test-api", True), ("database-migration-lambda", True)):
            with self.subTest(package=package):
                self.args.legacy_package = package
                commands = BENCHMARK.commands(self.args, self.root, self.target, self.output)
                self.assertEqual(commands[0][1][2:4], ["--legacy-package", package])
                self.assertNotIn("--features", commands[0][1])
                self.assertEqual(any(name == "SUPERVISOR_COMPILE" for name, _ in commands), supervisor)


if __name__ == "__main__":
    unittest.main()
