import importlib.util
import pathlib
import sys
import types
import unittest

CI = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(CI))
SPEC = importlib.util.spec_from_file_location("lane_benchmark", CI / "bench-test-lane.py")
BENCHMARK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BENCHMARK)


class LaneMeasurementCommands(unittest.TestCase):
    def setUp(self):
        self.args = types.SimpleNamespace(lane=["INFRASTRUCTURE", "REMAINING"], harness="libtest", slots=2,
            profile="api", features=["test-api/s3,test-api/ses"], legacy_package=None)
        self.root, self.target, self.output = map(pathlib.Path, ["/checkout", "/target", "/evidence"])

    def test_compile_execution_and_coverage_share_one_target_and_feature_configuration(self):
        commands = BENCHMARK.commands(self.args, self.root, self.target, self.output)
        self.assertEqual([name for name, _ in commands], ["COMPILE_LINK", "SUPERVISOR_COMPILE", "TESTS_AND_FIXTURES", "COVERAGE"])
        compile_command, execute_command = commands[0][1], commands[2][1]
        self.assertEqual(compile_command, [*execute_command, "--list-only"])
        self.assertIn("/target/debug/aura-test-env", execute_command)
        self.assertIn("/evidence/state", execute_command)
        self.assertIn("/evidence/discovery", execute_command)
        self.assertIn("test-api/s3,test-api/ses", execute_command)
        self.assertEqual(execute_command[2:6], ["--lane", "INFRASTRUCTURE", "--lane", "REMAINING"])
        self.assertEqual(commands[-1][1], ["bash", "/checkout/ci/collect-coverage.sh", "/evidence/profraw", "/target/debug/deps", "/evidence/coverage.lcov"])

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
