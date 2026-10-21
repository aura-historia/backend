#!/usr/bin/env python3
"""Exercise the Rust job supervisor against isolated real Docker resources."""
import argparse
import concurrent.futures
import json
import os
import pathlib
import shutil
import signal
import subprocess
import sys
import tempfile
import time

PREFIX = "AURA_TEST_ENV "
LABEL = "io.aura-historia.test-env.v1"
DOCKER = ["docker", "--host", "unix:///var/run/docker.sock"]


def child(binary, count):
    assert os.environ["AWS_ACCESS_KEY_ID"] == "test"
    assert os.environ["AWS_SECRET_ACCESS_KEY"] == "test"
    assert os.environ["AWS_EC2_METADATA_DISABLED"] == "true"
    assert "AWS_PROFILE" not in os.environ
    assert "AWS_ENDPOINT_URL_S3" not in os.environ
    assert pathlib.Path(os.environ["AWS_CONFIG_FILE"]).read_bytes() == b""
    config = json.loads(pathlib.Path(os.environ["AURA_TEST_ENV_CONFIG"]).read_text())
    assert config["schema_version"] == 1 and config["slots"] == 4
    # Attachment must work without Docker in the test process's PATH.
    env = dict(os.environ, PATH="/aura-test-env-no-docker")
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as executor:
        futures = [executor.submit(subprocess.run, [binary, "probe"], env=env,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30)
                   for _ in range(count)]
        for future in futures:
            result = future.result()
            assert result.returncode == 0, result.stderr.decode()
    # A supplied invalid contract must fail instead of starting unmanaged services.
    invalid = dict(env, AURA_TEST_ENV_CONFIG="/absent/environment.json")
    assert subprocess.run([binary, "probe"], env=invalid, capture_output=True).returncode != 0


class Suite:
    def __init__(self, binary, directory):
        self.binary = str(pathlib.Path(binary).resolve())
        self.root = pathlib.Path(directory)
        self.state = self.root / "state"
        self.handles = []
        self.runs = set()
        self.results = []

    def start(self, profile, command=None, env=None):
        number = len(self.handles)
        log = self.root / f"run-{number}.log"
        stream = log.open("w")
        command = command or [sys.executable, "-c", "import time; time.sleep(300)"]
        environment = dict(os.environ, AWS_PROFILE="fake-canary-profile",
                           AWS_ENDPOINT_URL_S3="https://never-contact.invalid",
                           AWS_ACCESS_KEY_ID="fake-canary-key")
        environment.update(env or {})
        process = subprocess.Popen([self.binary, "run", "--profile", profile, "--slots", "4",
                                    "--state-dir", str(self.state), "--", *command],
                                   env=environment, stdout=stream, stderr=subprocess.STDOUT)
        handle = (process, log, stream)
        self.handles.append(handle)
        return handle

    def events(self, handle):
        values = [json.loads(line[len(PREFIX):]) for line in handle[1].read_text().splitlines()
                  if line.startswith(PREFIX)]
        self.runs.update(event["run_id"] for event in values)
        return values

    def ready(self, handle, event="READY"):
        deadline = time.monotonic() + 180
        while time.monotonic() < deadline:
            values = self.events(handle)
            match = next((value for value in values if value["event"] == event), None)
            if match:
                return match
            assert handle[0].poll() is None, handle[1].read_text()
            time.sleep(0.05)
        raise AssertionError("supervisor readiness deadline exceeded: " + handle[1].read_text())

    def finish(self, handle, expected=0):
        assert handle[0].wait(timeout=180) == expected, handle[1].read_text()
        events = self.events(handle)
        assert any(value["event"] == "CLEANUP_COMPLETE" for value in events), handle[1].read_text()
        starts = [value["service"] for value in events if value["event"] == "SERVICE_STARTED"]
        assert len(starts) == len(set(starts)), "a service started more than once"
        return starts

    def gc(self):
        subprocess.run([self.binary, "gc", "--state-dir", str(self.state)], check=True,
                       capture_output=True, timeout=60)

    def probe(self, ready):
        subprocess.run([self.binary, "probe"], check=True, capture_output=True, timeout=30,
                       env=dict(os.environ, AURA_TEST_ENV_MANAGED="1", AURA_TEST_ENV_CONFIG=ready["config"]))

    def profiles(self):
        for profile, count, expected in [("postgres", 32, {"postgres"}),
                                          ("aws", 256, {"floci"}),
                                          ("opensearch", 16, {"floci", "opensearch"})]:
            command = [sys.executable, str(pathlib.Path(__file__).resolve()), "--binary", self.binary,
                       "--child-probes", str(count)]
            handle = self.start(profile, command)
            starts = self.finish(handle)
            assert set(starts) == expected
            self.results.append({"profile": profile, "attach_processes": count, "services": starts})

    def concurrent_cancellation(self):
        first, second = self.start("api"), self.start("api")
        one, two = self.ready(first), self.ready(second)
        assert one["run_id"] != two["run_id"] and one["config"] != two["config"]
        a = json.loads(pathlib.Path(one["config"]).read_text())
        b = json.loads(pathlib.Path(two["config"]).read_text())
        assert a["postgres"]["port"] != b["postgres"]["port"]
        assert a["aws_endpoint"] != b["aws_endpoint"]
        assert a["opensearch_endpoint"] != b["opensearch_endpoint"]
        self.gc()  # Both live run locks must be respected.
        first[0].send_signal(signal.SIGTERM)
        self.finish(first, 128 + signal.SIGTERM)
        self.probe(two)
        # A killed supervisor leaves only its own resources for safe stale GC.
        second[0].kill()
        assert second[0].wait(timeout=10) == -signal.SIGKILL
        self.gc()
        self.results.append({"parallel_api_supervisors": 2, "cancellation_and_stale_gc": "PASSED"})

    def failures(self):
        for command, expected in [([sys.executable, "-c", "raise SystemExit(37)"], 37),
                                   ([sys.executable, "-c", "import os,resource,signal; resource.setrlimit(resource.RLIMIT_CORE,(0,0)); os.kill(os.getpid(),signal.SIGABRT)"], 128 + signal.SIGABRT)]:
            self.finish(self.start("postgres", command), expected)
        interrupted = self.start("postgres")
        self.ready(interrupted)
        interrupted[0].send_signal(signal.SIGINT)
        self.finish(interrupted, 128 + signal.SIGINT)
        bootstrap = self.start("postgres")
        self.ready(bootstrap, "SERVICE_STARTED")
        bootstrap[0].send_signal(signal.SIGTERM)
        self.finish(bootstrap, 128 + signal.SIGTERM)
        restarted = self.start("postgres")
        ready = self.ready(restarted)
        subprocess.run([*DOCKER, "restart", f"aura-test-{ready['run_id']}-postgres"],
                       check=True, capture_output=True, timeout=30)
        self.finish(restarted, 1)
        # Fail after Docker create: partial bootstrap must still clean its objects.
        shim = self.root / "shim"
        shim.mkdir()
        real = shutil.which("docker")
        (shim / "docker").write_text(f"#!/bin/sh\nif [ \"$3\" = start ]; then exit 93; fi\nexec '{real}' \"$@\"\n")
        (shim / "docker").chmod(0o700)
        failed = self.start("postgres", env={"PATH": str(shim) + os.pathsep + os.environ["PATH"]})
        self.finish(failed, 1)
        # A failed cleanup retains its ownership record, and a later GC can retry it.
        cleanup_shim = self.root / "cleanup-shim"
        cleanup_shim.mkdir()
        denied = self.root / "deny-cleanup"
        denied.touch()
        (cleanup_shim / "docker").write_text(f"#!/bin/sh\nif [ \"$3\" = rm ] && [ -f '{denied}' ]; then exit 94; fi\nexec '{real}' \"$@\"\n")
        (cleanup_shim / "docker").chmod(0o700)
        failed_cleanup = self.start("postgres", [sys.executable, "-c", "pass"],
                                    env={"PATH": str(cleanup_shim) + os.pathsep + os.environ["PATH"]})
        assert failed_cleanup[0].wait(timeout=60) == 1, failed_cleanup[1].read_text()
        events = self.events(failed_cleanup)
        ready = next(value for value in events if value["event"] == "READY")
        assert pathlib.Path(ready["config"]).exists()
        assert not any(value["event"] == "CLEANUP_COMPLETE" for value in events)
        denied.unlink()
        self.gc()
        self.results.append({"exit_crash_sigint_bootstrap_cancel_restart_docker_failure": "PASSED"})

    def descendants(self):
        pid_path = self.root / "descendant.pid"
        script = "import pathlib,subprocess; p=subprocess.Popen(['sleep','300']); pathlib.Path(" + repr(str(pid_path)) + ").write_text(str(p.pid))"
        self.finish(self.start("postgres", [sys.executable, "-c", script]))
        pid = int(pid_path.read_text())
        stat = pathlib.Path(f"/proc/{pid}/stat")
        assert not stat.exists(), "supervisor must reap orphaned descendants"
        self.results.append({"orphaned_child_descendants": "REAPED"})

    def fixture(self, binary):
        if binary:
            self.finish(self.start("postgres", [binary, "--exact", "managed_fixture_child", "--nocapture"]))
            self.results.append({"managed_business_migrations_extensions_tls": "PASSED"})

    def assert_clean(self):
        self.gc()
        for run in self.runs:
            for command in [["ps", "-aq"], ["volume", "ls", "-q"], ["network", "ls", "-q"]]:
                found = subprocess.check_output([*DOCKER, *command, "--filter", f"label={LABEL}.run={run}"])
                assert found.strip() == b"", f"owned resources leaked for {run}"
        assert not list(self.state.iterdir()), "run state leaked"

    def close(self):
        for process, _, stream in self.handles:
            if process.poll() is None:
                process.send_signal(signal.SIGTERM)
                try:
                    process.wait(timeout=20)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=10)
            stream.close()
        self.gc()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--fixture-binary")
    parser.add_argument("--child-probes", type=int)
    args = parser.parse_args()
    if args.child_probes:
        child(args.binary, args.child_probes)
        return
    with tempfile.TemporaryDirectory(prefix="aura-test-env-smoke-") as directory:
        suite = Suite(args.binary, directory)
        try:
            suite.profiles()
            suite.concurrent_cancellation()
            suite.failures()
            suite.descendants()
            suite.fixture(args.fixture_binary)
            suite.assert_clean()
            print(json.dumps({"status": "PASSED", "runs": len(suite.runs), "scenarios": suite.results}))
        except Exception:
            for _, log, _ in suite.handles:
                print(log.name + ":\n" + log.read_text(), file=sys.stderr)
            raise
        finally:
            suite.close()


if __name__ == "__main__":
    main()
