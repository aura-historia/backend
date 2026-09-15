"""C1 CI prerequisite and lightweight-runner regressions; no real daemon or registry calls."""
import io
import json
import shutil
import stat
import subprocess
import tempfile
import unittest
from contextlib import redirect_stderr
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import run_ci

ROOT = Path(__file__).resolve().parents[2]
HELPER = ROOT / ".github/scripts/prepare-postgres-test-image.sh"
LOCAL_ENDPOINT = "unix:///var/run/docker.sock"
CANARY = "private-provider-body-canary"
IMAGE = "ghcr.io/aura-historia/test-postgres:synthetic"
IMAGE_ID = "sha256:" + "a" * 64

FAKE_DOCKER = r'''#!/usr/bin/env python3
import json
import os
from pathlib import Path
import sys

root = Path(__file__).parent
args = sys.argv[1:]
with (root / "calls").open("a", encoding="utf-8") as calls:
    calls.write(json.dumps(args) + "\n")
(root / "environment.json").write_text(json.dumps(dict(os.environ)), encoding="utf-8")

if args[2] == "pull":
    status = int(os.environ.get("FAKE_PULL_STATUS", "0"))
    stderr = os.environ.get("FAKE_PULL_STDERR", "")
    if stderr:
        sys.stderr.write(stderr)
    raise SystemExit(status)
if args[2:4] == ["image", "inspect"]:
    status = int(os.environ.get("FAKE_INSPECT_STATUS", "0"))
    stdout = os.environ.get("FAKE_INSPECT_STDOUT", "")
    stderr = os.environ.get("FAKE_INSPECT_STDERR", "")
    if stdout:
        sys.stdout.write(stdout)
    if stderr:
        sys.stderr.write(stderr)
    raise SystemExit(status)
raise SystemExit("unexpected fake Docker operation")
'''


class HelperFixture:
    def __init__(self, reference=IMAGE):
        self.root = Path(tempfile.mkdtemp(prefix="aura-ci-helper-"))
        scripts = self.root / ".github/scripts"
        scripts.mkdir(parents=True)
        shutil.copy2(HELPER, scripts / HELPER.name)
        reference_path = self.root / "src/test-api/postgres/image-ref.txt"
        reference_path.parent.mkdir(parents=True)
        reference_path.write_text(reference, encoding="utf-8")
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.docker = self.bin / "docker"
        self.docker.write_text(FAKE_DOCKER, encoding="utf-8")
        self.docker.chmod(self.docker.stat().st_mode | stat.S_IXUSR)
        self.env_file = self.root / "github-env"
        self.runner_temp = self.root / "runner-temp"
        self.runner_temp.mkdir()

    def run(self, extra_environment=None, **settings):
        environment = {
            "PATH": f"{self.bin}:/usr/bin:/bin",
            "GITHUB_ENV": str(self.env_file),
            "RUNNER_TEMP": str(self.runner_temp),
            "FAKE_PULL_STATUS": "0",
            "FAKE_INSPECT_STATUS": "0",
            "FAKE_INSPECT_STDOUT": IMAGE_ID,
        }
        for name, value in settings.items():
            environment["FAKE_" + name.upper()] = str(value)
        environment.update(extra_environment or {})
        return subprocess.run(
            ["bash", str(self.root / ".github/scripts" / HELPER.name)],
            cwd=self.root,
            env=environment,
            text=True,
            capture_output=True,
            check=False,
            timeout=10,
        )

    def calls(self):
        path = self.bin / "calls"
        if not path.exists():
            return []
        return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines()]

    def environment(self):
        return json.loads((self.bin / "environment.json").read_text(encoding="utf-8"))

    def env_contents(self):
        return self.env_file.read_text(encoding="utf-8") if self.env_file.exists() else ""

    def close(self):
        shutil.rmtree(self.root)


class PostgresImagePreparationTests(unittest.TestCase):
    def setUp(self):
        self.fixtures = []

    def tearDown(self):
        for fixture in self.fixtures:
            fixture.close()

    def fixture(self, reference=IMAGE):
        fixture = HelperFixture(reference)
        self.fixtures.append(fixture)
        return fixture

    def assert_no_override(self, fixture):
        self.assertNotIn("AURA_TEST_POSTGRES_IMAGE=", fixture.env_contents())

    def assert_required_calls(self, fixture, reference=IMAGE):
        self.assertEqual(
            fixture.calls(),
            [
                ["--host", LOCAL_ENDPOINT, "pull", reference],
                ["--host", LOCAL_ENDPOINT, "image", "inspect", "--format", "{{.Id}}", reference],
            ],
        )

    def test_valid_reference_pulls_and_exports_inspected_local_id(self):
        fixture = self.fixture()
        result = fixture.run()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assert_required_calls(fixture)
        self.assertEqual(fixture.env_contents(), f"AURA_TEST_POSTGRES_IMAGE={IMAGE_ID}\n")
        self.assertNotIn(CANARY, result.stdout + result.stderr + fixture.env_contents())

    def test_invalid_reference_fails_before_any_docker_call(self):
        for reference in ("", "first\nsecond", " leading", "--pull=always", "image\tname"):
            with self.subTest(reference=repr(reference)):
                fixture = self.fixture(reference)
                result = fixture.run()
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(fixture.calls(), [])
                self.assert_no_override(fixture)

    def test_pull_failure_stops_before_inspect_without_fallback(self):
        fixture = self.fixture()
        result = fixture.run(pull_status=1, pull_stderr=CANARY)
        self.assertEqual(result.returncode, 1)
        self.assertEqual(len(fixture.calls()), 1)
        self.assertEqual(fixture.calls()[0], ["--host", LOCAL_ENDPOINT, "pull", IMAGE])
        self.assert_no_override(fixture)
        self.assertNotIn(CANARY, result.stdout + result.stderr + fixture.env_contents())

    def test_pull_timeout_status_is_failure_without_cleanup_authority(self):
        fixture = self.fixture()
        result = fixture.run(pull_status=124, pull_stderr=CANARY)
        self.assertEqual(result.returncode, 124)
        self.assertIn("exit=124", result.stderr)
        self.assertEqual(len(fixture.calls()), 1)
        self.assert_no_override(fixture)
        self.assertNotIn(CANARY, result.stdout + result.stderr + fixture.env_contents())

    def test_inspect_failure_and_malformed_id_stop_without_override(self):
        for settings in (
            {"inspect_status": 1, "inspect_stderr": CANARY},
            {"inspect_stdout": "not-a-local-image-id", "inspect_stderr": CANARY},
        ):
            with self.subTest(settings=settings):
                fixture = self.fixture()
                result = fixture.run(**settings)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(len(fixture.calls()), 2)
                self.assert_no_override(fixture)
                self.assertNotIn(CANARY, result.stdout + result.stderr + fixture.env_contents())

    def test_remote_docker_environment_cannot_change_explicit_target(self):
        fixture = self.fixture()
        result = fixture.run(
            extra_environment={
                "DOCKER_HOST": "tcp://remote.invalid:2376",
                "DOCKER_CONTEXT": "remote-context",
                "DOCKER_TLS_VERIFY": "1",
                "DOCKER_CERT_PATH": "/untrusted/certificates",
                "DOCKER_TLS": "1",
                "DOCKER_API_VERSION": CANARY,
            }
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assert_required_calls(fixture)
        self.assertNotIn(CANARY, result.stdout + result.stderr + fixture.env_contents())
        child_environment = fixture.environment()
        for name in (
            "DOCKER_HOST",
            "DOCKER_CONTEXT",
            "DOCKER_TLS_VERIFY",
            "DOCKER_CERT_PATH",
            "DOCKER_TLS",
            "DOCKER_API_VERSION",
        ):
            self.assertNotIn(name, child_environment)

    def test_only_pull_and_inspect_are_recorded(self):
        fixture = self.fixture()
        result = fixture.run()
        self.assertEqual(result.returncode, 0, result.stderr)
        operations = [call[2] for call in fixture.calls()]
        self.assertEqual(operations, ["pull", "image"])
        self.assertNotIn(
            "create",
            operations + [argument for call in fixture.calls() for argument in call[3:]],
        )
        self.assertNotIn("run", operations)
        self.assertNotIn("rm", operations)
        self.assertNotIn("prune", operations)
        self.assertNotIn("push", operations)


class _TestIdentity:
    def __init__(self, name):
        self.name = name

    def id(self):
        return self.name


def result_stub(*, skipped=None, successful=True, unexpected_successes=None, tests_run=2):
    return SimpleNamespace(
        skipped=list(skipped or []),
        failures=[] if successful else [("failure", "details")],
        errors=[],
        unexpectedSuccesses=list(unexpected_successes or []),
        testsRun=tests_run,
        wasSuccessful=lambda: successful,
    )


class LightweightRunnerTests(unittest.TestCase):
    def allowed_skip(self):
        return [(_TestIdentity(run_ci.EXPECTED_SKIPPED_ID), run_ci.EXPECTED_SKIP_REASON)]

    def test_accepts_success_with_the_one_documented_skip(self):
        result = result_stub(skipped=self.allowed_skip())
        self.assertTrue(run_ci.result_is_accepted(result, 2))

    def test_rejects_failure_or_error_even_with_allowed_skip(self):
        for kind in ("failure", "error"):
            with self.subTest(kind=kind):
                result = result_stub(skipped=self.allowed_skip(), successful=False)
                if kind == "error":
                    result.failures = []
                    result.errors = [("error", "details")]
                self.assertFalse(run_ci.result_is_accepted(result, 2))

    def test_rejects_zero_or_changed_execution_count(self):
        result = result_stub(skipped=self.allowed_skip(), tests_run=0)
        self.assertFalse(run_ci.result_is_accepted(result, 2))

    def test_rejects_unexpected_skip_or_success(self):
        unexpected_skip = self.allowed_skip() + [(_TestIdentity("test.other"), "new reason")]
        self.assertFalse(run_ci.result_is_accepted(result_stub(skipped=unexpected_skip), 2))
        self.assertFalse(
            run_ci.result_is_accepted(
                result_stub(skipped=self.allowed_skip(), unexpected_successes=[_TestIdentity("test.other")]),
                2,
            )
        )

    def test_requires_compose_opt_in(self):
        with patch.dict(run_ci.os.environ, {}, clear=True), redirect_stderr(io.StringIO()):
            self.assertEqual(run_ci.main(), 2)

    def test_rejects_cached_helper_opt_in_in_c1(self):
        environment = {"AURA_TEST_COMPOSE_CONFIG": "1", "AURA_TEST_CA_MOUNT": "1"}
        with patch.dict(run_ci.os.environ, environment, clear=True), redirect_stderr(io.StringIO()):
            self.assertEqual(run_ci.main(), 2)

    def test_zero_discovery_fails(self):
        environment = {"AURA_TEST_COMPOSE_CONFIG": "1"}
        with patch.dict(run_ci.os.environ, environment, clear=True), patch.object(
            run_ci.unittest.TestLoader, "discover", return_value=unittest.TestSuite()
        ), redirect_stderr(io.StringIO()):
            self.assertEqual(run_ci.main(), 1)


if __name__ == "__main__":
    unittest.main()
