import contextlib
import importlib.util
import io
import json
import os
import pathlib
import signal
import subprocess
import sys
import tempfile
import time
import types
import unittest
from unittest.mock import patch

CI = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(CI))
SPEC = importlib.util.spec_from_file_location("lane_driver", CI / "run-test-lane.py")
DRIVER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(DRIVER)


class LaneDriverExecution(unittest.TestCase):
    def test_child_uses_its_own_process_group_and_inherits_coverage(self):
        with tempfile.TemporaryDirectory() as directory:
            destination = pathlib.Path(directory) / "child.json"
            child = "import json,os,pathlib,sys; pathlib.Path(sys.argv[1]).write_text(json.dumps([os.getpid(),os.getpgrp(),os.environ['LLVM_PROFILE_FILE']]))"
            before = {number: signal.getsignal(number) for number in (signal.SIGINT, signal.SIGTERM)}
            DRIVER.execute([sys.executable, "-c", child, str(destination)], directory,
                           dict(os.environ, LLVM_PROFILE_FILE="/fixture/%p-%m.profraw"))
            pid, group, profiles = json.loads(destination.read_text())
            self.assertEqual(pid, group)
            self.assertNotEqual(group, os.getpgrp())
            self.assertEqual(profiles, "/fixture/%p-%m.profraw")
            self.assertEqual({number: signal.getsignal(number) for number in before}, before)

    def test_child_failure_keeps_original_status_and_restores_signal_handlers(self):
        before = {number: signal.getsignal(number) for number in (signal.SIGINT, signal.SIGTERM)}
        with self.assertRaises(SystemExit) as failure:
            DRIVER.execute([sys.executable, "-c", "raise SystemExit(37)"], CI, os.environ)
        self.assertEqual(failure.exception.code, 37)
        self.assertEqual({number: signal.getsignal(number) for number in before}, before)

    def test_spawn_failure_restores_signal_handlers(self):
        before = {number: signal.getsignal(number) for number in (signal.SIGINT, signal.SIGTERM)}
        with tempfile.TemporaryDirectory() as directory, self.assertRaises(FileNotFoundError):
            DRIVER.execute([str(pathlib.Path(directory) / "missing")], directory, os.environ)
        self.assertEqual({number: signal.getsignal(number) for number in before}, before)

    def test_cancellation_during_handler_restoration_cannot_return_success(self):
        before = {number: signal.getsignal(number) for number in (signal.SIGINT, signal.SIGTERM)}
        install = signal.signal

        def restore(number, handler):
            if number == signal.SIGINT and handler == before[number]:
                signal.getsignal(signal.SIGTERM)(signal.SIGTERM, None)
            return install(number, handler)

        with patch.object(DRIVER.signal, "signal", side_effect=restore), self.assertRaises(SystemExit) as failure:
            DRIVER.execute([sys.executable, "-c", "pass"], CI, os.environ)
        self.assertEqual(failure.exception.code, 128 + signal.SIGTERM)
        self.assertEqual({number: signal.getsignal(number) for number in before}, before)

    def test_cancellation_reaches_owned_descendants_and_waits_for_cleanup(self):
        grandchild = """
import pathlib,signal,sys,time
directory=pathlib.Path(sys.argv[1])
def stop(number, frame):
    (directory/'grandchild-signal').write_text(str(number))
    raise SystemExit(0)
signal.signal(signal.SIGINT,stop)
signal.signal(signal.SIGTERM,stop)
(directory/'grandchild-ready').touch()
while True: time.sleep(.02)
"""
        child = """
import os,pathlib,signal,subprocess,sys,time
directory=pathlib.Path(sys.argv[1])
grandchild=subprocess.Popen([sys.executable,'-c',sys.argv[2],str(directory)])
def stop(number, frame):
    (directory/'child-signal').write_text(str(number))
    raise SystemExit(0)
signal.signal(signal.SIGINT,stop)
signal.signal(signal.SIGTERM,stop)
(directory/'child-pid').write_text(str(os.getpid()))
(directory/'child-ready').touch()
try:
    while True: time.sleep(.02)
finally:
    try: grandchild.wait(timeout=3)
    except subprocess.TimeoutExpired:
        grandchild.kill()
        grandchild.wait()
    (directory/'child-cleanup').touch()
"""
        wrapper = """
import importlib.util,os,pathlib,sys
ci=pathlib.Path(sys.argv[1]);sys.path.insert(0,str(ci))
spec=importlib.util.spec_from_file_location('driver',ci/'run-test-lane.py')
driver=importlib.util.module_from_spec(spec);spec.loader.exec_module(driver)
driver.execute([sys.executable,'-c',sys.argv[2],sys.argv[3],sys.argv[4]],sys.argv[3],os.environ)
"""
        for number in (signal.SIGINT, signal.SIGTERM):
            with self.subTest(signal=number), tempfile.TemporaryDirectory() as temporary:
                directory = pathlib.Path(temporary)
                process = subprocess.Popen([sys.executable, "-c", wrapper, str(CI), child, temporary, grandchild],
                                           stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                try:
                    deadline = time.monotonic() + 10
                    while not all((directory / name).exists() for name in ("child-ready", "grandchild-ready")):
                        if process.poll() is not None or time.monotonic() >= deadline:
                            self.fail("Python process probe did not become ready")
                        time.sleep(.01)
                    process.send_signal(number)
                    stdout, stderr = process.communicate(timeout=10)
                    self.assertEqual(process.returncode, 128 + number, stdout + stderr)
                    self.assertEqual((directory / "child-signal").read_text(), str(number))
                    self.assertEqual((directory / "grandchild-signal").read_text(), str(number))
                    self.assertTrue((directory / "child-cleanup").is_file())
                finally:
                    if process.poll() is None:
                        process.kill()
                    process.communicate()
                    pid_file = directory / "child-pid"
                    if pid_file.is_file() and not (directory / "child-cleanup").is_file():
                        try:
                            os.killpg(int(pid_file.read_text()), signal.SIGKILL)
                        except ProcessLookupError:
                            pass


class LaneDriverPhases(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = pathlib.Path(self.temporary.name)
        self.args = types.SimpleNamespace(phase="all", output=self.root / "output", state_dir=self.root / "state",
            supervisor=pathlib.Path("/fixture/aura-test-env"), profile="api", slots=2, features=[],
            harness="nextest", lane=["INFRASTRUCTURE"], list_only=False)
        self.selection = {"packages": ["database-migration-lambda", "test-api"],
                          "package_args": ["--package", "database-migration-lambda", "--package", "test-api"]}
        self.suites = {DRIVER.PRISTINE_TARGET: {}}

    def test_pristine_precedes_ordinary_and_unmanaged_controls_preserve_coverage(self):
        with patch.object(DRIVER, "arguments", return_value=self.args), patch.object(DRIVER, "command", return_value=str(self.root)), patch.object(
                DRIVER, "discover_lane", return_value=(self.selection, self.suites)), patch.object(DRIVER, "execute") as execute, patch.dict(
                os.environ, {"AURA_TEST_ENV_MANAGED": "1", "NEXTEST": "1", "LLVM_PROFILE_FILE": "/fixture/%p-%m.profraw"}, clear=True):
            DRIVER.main()
        commands = [call.args[0] for call in execute.call_args_list]
        self.assertTrue(self.args.state_dir.is_dir())
        self.assertEqual(len(commands), 4)
        self.assertEqual(commands[0][:6], ["/fixture/aura-test-env", "run", "--profile", "postgres", "--slots", "1"])
        self.assertIn(str(self.args.state_dir / "pristine"), commands[0])
        self.assertEqual(commands[1][:6], ["/fixture/aura-test-env", "run", "--profile", "api", "--slots", "2"])
        self.assertIn(str(self.args.state_dir / "managed"), commands[1])
        self.assertEqual(commands[1][-2:], ["--phase", "managed"])
        self.assertEqual(commands[2][-6:], ["--test", "aws", "--test", "managed_environment", "--", "--test-threads=1"])
        self.assertIn("should_verify_managed_context_isolation_and_recovery", commands[3])
        for call in execute.call_args_list:
            environment = call.args[2]
            self.assertNotIn("AURA_TEST_ENV_MANAGED", environment)
            self.assertNotIn("NEXTEST", environment)
            self.assertEqual(environment["LLVM_PROFILE_FILE"], "/fixture/%p-%m.profraw")

    def test_failure_in_any_phase_prevents_later_phases(self):
        for failing_index in range(4):
            with self.subTest(phase=failing_index):
                outcomes = [None] * failing_index + [SystemExit(37)]
                with patch.object(DRIVER, "arguments", return_value=self.args), patch.object(DRIVER, "command", return_value=str(self.root)), patch.object(
                        DRIVER, "discover_lane", return_value=(self.selection, self.suites)), patch.object(DRIVER, "execute", side_effect=outcomes) as execute:
                    with self.assertRaises(SystemExit) as failure:
                        DRIVER.main()
                self.assertEqual(failure.exception.code, 37)
                self.assertEqual(execute.call_count, failing_index + 1)

    def test_list_only_validates_discovery_without_running_a_supervisor_or_body(self):
        self.args.list_only = True
        with patch.object(DRIVER, "arguments", return_value=self.args), patch.object(DRIVER, "command", return_value=str(self.root)), patch.object(
                DRIVER, "discover_lane", return_value=(self.selection, self.suites)) as discover, patch.object(DRIVER, "execute") as execute:
            DRIVER.main()
        discover.assert_called_once_with(self.args, self.root)
        execute.assert_not_called()

    def test_managed_tests_precede_explicit_doctests(self):
        with patch.object(DRIVER, "execute") as execute:
            DRIVER.managed_phase(self.args, self.root, self.selection, {})
        self.assertEqual(execute.call_args_list[0].args[0][:3], ["cargo", "nextest", "run"])
        self.assertEqual(execute.call_args_list[1].args[0], ["cargo", "test", "--locked", "--all-features",
            "--package", "database-migration-lambda", "--package", "test-api", "--doc"])

    def test_invalid_capacity_or_service_profile_fails_before_discovery(self):
        base = ["run-test-lane.py", "--lane", "INFRASTRUCTURE", "--harness", "nextest",
                "--supervisor", "/fixture/supervisor", "--state-dir", str(self.root), "--output", str(self.root)]
        for options in (["--slots", "0", "--profile", "api"], ["--slots", "1025", "--profile", "api"],
                        ["--slots", "2", "--profile", "none"]):
            with self.subTest(options=options), patch.object(sys, "argv", [*base, *options]), contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as failure:
                    DRIVER.arguments()
                self.assertEqual(failure.exception.code, 2)


if __name__ == "__main__":
    unittest.main()
