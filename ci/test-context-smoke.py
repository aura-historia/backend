#!/usr/bin/env python3
"""Focused isolation/recovery checks using the job-owned Rust supervisor."""
import argparse
import json
import os
import pathlib
import subprocess
import sys
import time
import uuid


def wait_file(path, process):
    deadline = time.monotonic() + 90
    while not path.exists():
        assert process.poll() is None, f"child exited before readiness: {process.returncode}"
        assert time.monotonic() < deadline, f"context readiness timed out: {path}"
        time.sleep(0.02)
    return json.loads(path.read_text())


def inside(fixture, output):
    config = json.loads(pathlib.Path(os.environ["AURA_TEST_ENV_CONFIG"]).read_text())
    slots = config["slots"]
    required = ["context_fixture_child", "mail_sentinel_child", "cleanup_failure_child"]
    if slots == 4:
        required.append("reset_benchmark_child")
    listing = subprocess.run([fixture, "--list"], capture_output=True, text=True,
                             check=True, timeout=30).stdout.splitlines()
    missing = [name for name in required if f"{name}: test" not in listing]
    assert not missing, f"fixture executable is missing required probes: {missing}"
    children = []
    streams = []

    def start(label, slot, release, action="exercise", previous=None, invocation=None, explicit=False):
        report = output / f"{label}.json"
        env = dict(os.environ, AURA_CONTEXT_ACTION=action, AURA_CONTEXT_OUTPUT=str(report),
                   AURA_CONTEXT_RELEASE=str(release), NEXTEST="1", NEXTEST_RUN_ID=invocation or str(uuid.uuid4()),
                   NEXTEST_TEST_GLOBAL_SLOT=str(slot), NEXTEST_EXECUTION_MODE="process-per-test")
        if previous:
            env["AURA_CONTEXT_PREVIOUS"] = str(previous)
        if explicit:
            env["AURA_CONTEXT_EXPLICIT"] = "1"
        stream = (output / f"{label}.log").open("w")
        streams.append(stream)
        process = subprocess.Popen([fixture, "--exact", "context_fixture_child", "--nocapture"],
                                   env=env, stdout=stream, stderr=subprocess.STDOUT)
        children.append(process)
        return process, report

    def finish(process):
        assert process.wait(timeout=120) == 0, f"context failed; inspect {output}"

    def single(test, setting):
        env = dict(os.environ, **setting)
        result = subprocess.run([fixture, "--exact", test, "--nocapture"], env=env,
                                capture_output=True, text=True, timeout=120)
        (output / f"{test}-{next(iter(setting.values()))}.log").write_text(result.stdout + result.stderr)
        assert result.returncode == 0, result.stdout + result.stderr

    try:
        single("mail_sentinel_child", {"AURA_CONTEXT_SENTINEL": "create"})
        release = output / "parallel.release"
        invocation = str(uuid.uuid4())
        probes = [start(f"parallel-{slot}", slot, release, invocation=invocation) for slot in range(slots)]
        reports = [wait_file(report, process) for process, report in probes]
        assert len({report["database"] for report in reports}) == slots
        assert len({report["index"] for report in reports}) == slots
        assert len({report["sender"] for report in reports}) == slots
        assert {report["slot"] for report in reports} == set(range(slots))
        release.touch()
        for process, _ in probes:
            finish(process)

        explicit_release = output / "explicit.release"
        probes = [start(f"explicit-{slot}", slot, explicit_release, explicit=True) for slot in range(slots)]
        explicit_reports = [wait_file(report, process) for process, report in probes]
        assert {report["slot"] for report in explicit_reports} == set(range(slots))
        explicit_release.touch()
        for process, _ in probes:
            finish(process)

        first_release, second_release = output / "first.release", output / "second.release"
        first, first_report = start("invocation-a", 0, first_release)
        wait_file(first_report, first)
        second, second_report = start("invocation-b", 0, second_release)
        time.sleep(0.2)
        assert not second_report.exists(), "simultaneous invocations collided in slot zero"
        first_release.touch()
        finish(first)
        wait_file(second_report, second)
        second_release.touch()
        finish(second)

        dirty, dirty_report = start("killed-holder", 0, output / "never.release", action="dirty")
        previous = wait_file(dirty_report, dirty)
        dirty.kill()
        assert dirty.wait(timeout=10) < 0
        recovery_release = output / "recovery.release"
        recovery_release.touch()
        recovery, recovery_report = start("recovered-holder", 0, recovery_release, previous=dirty_report)
        recovered = wait_file(recovery_report, recovery)
        assert recovered["test"] != previous["test"]
        finish(recovery)

        for number in range(2):
            seed, seed_report = start(f"seed-{number}", 0, recovery_release, action="seed")
            wait_file(seed_report, seed)
            finish(seed)

        single("cleanup_failure_child", {"AURA_CONTEXT_CLEANUP_FAILURE": "1"})

        # Real Nextest exercises its own invocation/attempt/slot contract and a filtered retry.
        if slots == 4:
            retry_report = output / "nextest-retry.json"
            env = dict(os.environ, AURA_CONTEXT_ACTION="exercise", AURA_CONTEXT_OUTPUT=str(retry_report),
                       AURA_CONTEXT_PREVIOUS=str(retry_report), AURA_CONTEXT_RELEASE=str(recovery_release),
                       AURA_CONTEXT_FAIL_FIRST_ATTEMPT="1")
            for key in list(env):
                if key.startswith("NEXTEST"):
                    del env[key]
            result = subprocess.run(["cargo", "nextest", "run", "--locked", "-p", "test-api", "--all-features",
                                     "--test", "isolated_context", "--test-threads", str(slots), "--retries", "1",
                                     "-E", "test(context_fixture_child)"], env=env, capture_output=True, text=True, timeout=180)
            (output / "nextest-retry.log").write_text(result.stdout + result.stderr)
            assert result.returncode == 0, result.stdout + result.stderr
        single("mail_sentinel_child", {"AURA_CONTEXT_SENTINEL": "check"})
        if slots == 4:
            single("reset_benchmark_child", {"AURA_CONTEXT_BENCHMARK": "1"})
        print(json.dumps({"slots": slots, "parallel_contexts": slots, "cross_invocation_contention": "PASS",
                          "killed_holder_recovery": "PASS", "bounded_explicit_contexts": slots, "real_nextest_retry": slots == 4}))
    finally:
        for process in children:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=10)
        for stream in streams:
            stream.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary")
    parser.add_argument("--fixture-binary", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--inside", action="store_true")
    parser.add_argument("--slots", type=int, choices=[1, 2, 4])
    args = parser.parse_args()
    output = pathlib.Path(args.output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    if args.inside:
        inside(str(pathlib.Path(args.fixture_binary).resolve()), output)
        return
    assert args.binary, "--binary is required outside a managed supervisor"
    binary = str(pathlib.Path(args.binary).resolve())
    state = output / "state"
    for slots in [args.slots] if args.slots else [1, 2, 4]:
        directory = output / f"slots-{slots}"
        directory.mkdir()
        command = [binary, "run", "--profile", "api", "--slots", str(slots), "--state-dir", str(state), "--",
                   sys.executable, str(pathlib.Path(__file__).resolve()), "--inside", "--fixture-binary",
                   str(pathlib.Path(args.fixture_binary).resolve()), "--output", str(directory)]
        started = time.monotonic()
        result = subprocess.run(command, capture_output=True, text=True, timeout=360)
        (directory / "supervisor.log").write_text(result.stdout + result.stderr)
        assert result.returncode == 0, result.stdout + result.stderr
        events = [json.loads(line.removeprefix("AURA_TEST_ENV ")) for line in result.stderr.splitlines()
                  if line.startswith("AURA_TEST_ENV ")]
        starts = [event["service"] for event in events if event["event"] == "SERVICE_STARTED"]
        assert sorted(starts) == ["floci", "opensearch", "postgres"]
        run = next(event["run_id"] for event in events if event["event"] == "READY")
        assert any(event["event"] == "CLEANUP_COMPLETE" for event in events)
        for kind, command in [("container", ["ps", "-aq"]), ("volume", ["volume", "ls", "-q"]), ("network", ["network", "ls", "-q"])]:
            owned = subprocess.run(["docker", "--host", "unix:///var/run/docker.sock", *command,
                                    "--filter", f"label=io.aura-historia.test-env.v1.run={run}"], capture_output=True, text=True, check=True)
            assert not owned.stdout.strip(), f"owned {kind} leaked for {run}"
        print(json.dumps({"slots": slots, "run": run, "service_starts": starts,
                          "elapsed_seconds": round(time.monotonic() - started, 3), "cleanup": "PASS"}))


if __name__ == "__main__":
    main()
