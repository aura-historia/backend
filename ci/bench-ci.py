#!/usr/bin/env python3
"""Measure complete current package tests with explicit cache state and artifacts."""

import argparse
import datetime
import json
import os
import pathlib
import subprocess
import sys
import time

from ci_benchmark.execution import measure, environment_metadata, images
from ci_benchmark.metrics import coverage_totals, observations


def text(argv, root):
    return subprocess.check_output(argv, cwd=root, text=True).strip()


def arguments():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package", required=True)
    parser.add_argument("--scenario", choices=["LEAF", "API", "FIXTURE"], required=True)
    parser.add_argument("--cache", choices=["COLD_TARGET", "WARM_TARGET"], required=True)
    parser.add_argument("--target-dir", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    parser.add_argument("--repeat", type=int, default=1)
    parser.add_argument("--without-coverage", action="store_true", help="Explicitly record an uninstrumented diagnostic, not a coverage baseline")
    parser.add_argument("--job-queued-at", help="ISO timestamp from GitHub's job API; omit when unavailable")
    parser.add_argument("--job-started-at", help="ISO timestamp from GitHub's job API")
    parser.add_argument("--job-metadata", type=pathlib.Path, help="Current GitHub job metadata from the jobs API")
    return parser.parse_args()


def build_environment(args, target, profiles):
    env = dict(os.environ, CARGO_TARGET_DIR=str(target), CARGO_INCREMENTAL="0", CARGO_TERM_COLOR="never",
               CARGO_PROFILE_TEST_DEBUG="1", CARGO_BUILD_JOBS=os.environ.get("CARGO_BUILD_JOBS", "4"), AURA_CI_BENCHMARK_TIMING="1")
    if not args.without_coverage:
        env["RUSTFLAGS"] = "-C instrument-coverage"
        env["LLVM_PROFILE_FILE"] = str(profiles / "%p-%m.profraw")
    elif env.get("RUSTFLAGS") or env.get("LLVM_PROFILE_FILE"):
        raise ValueError("Uninstrumented diagnostics require RUSTFLAGS and LLVM_PROFILE_FILE to be unset")
    # These are the fixture's fake credentials. Child commands must never inherit
    # real cloud credentials through the standard AWS credential chain.
    env.update(AWS_ACCESS_KEY_ID="test", AWS_SECRET_ACCESS_KEY="test", AWS_SESSION_TOKEN="test",
               AWS_EC2_METADATA_DISABLED="true", AWS_DEFAULT_REGION="eu-central-1", AWS_REGION="eu-central-1")
    return env


def benchmark(args, root, artifact):
    metadata = json.loads(text(["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"], root))
    matches = [p for p in metadata["packages"] if p["name"] == args.package]
    if len(matches) != 1:
        raise ValueError(f"Not a unique workspace package: {args.package}")
    cwd = pathlib.Path(matches[0]["manifest_path"]).parent
    target = args.target_dir.resolve()
    target.mkdir(parents=True, exist_ok=True)
    populated = any(target.iterdir())
    if populated != (args.cache == "WARM_TARGET"):
        raise ValueError("COLD_TARGET requires an empty target directory; WARM_TARGET requires a previously built target directory")
    if args.output.exists() and any(args.output.iterdir()):
        raise ValueError(f"Output directory already contains evidence: {args.output}")
    args.output.mkdir(parents=True, exist_ok=True)
    profiles = args.output / "profraw"
    profiles.mkdir()
    env = build_environment(args, target, profiles)
    artifact.update({"revision": text(["git", "rev-parse", "HEAD"], root),
        "dirty": bool(text(["git", "status", "--porcelain"], root)),
        "scenario": {"id": args.scenario, "package": args.package, "repeat": args.repeat},
        "cache": {"mode": args.cache, "target_dir": str(target), "initially_populated": populated,
                  "definition": "Cargo target directory only; registry, OS page cache and Docker images are separately recorded and are not claimed cold"},
        "feature_scope": "PACKAGE_ALL_FEATURES_FROM_PACKAGE_DIRECTORY",
        "runner": environment_metadata(root, env), "images_before": images(root),
        "coverage": {"status": "NOT_REQUESTED" if args.without_coverage else "PENDING", "totals": None}})
    cargo = ["cargo", "test", "--locked", "--all-features"]
    stages = artifact["stages"]
    stages.append(measure("COMPILE_LINK", [*cargo, "--no-run", "--timings"], cwd, env, args.output, target))
    if stages[-1]["exit_code"]:
        return
    stages.append(measure("TESTS_AND_FIXTURES", [*cargo, "--", "--nocapture"], cwd, env, args.output, target))
    artifact["observations"] = observations(args.output / "tests_and_fixtures.log")
    if not args.without_coverage:
        lcov = args.output / "coverage.lcov"
        stages.append(measure("COVERAGE", ["grcov", str(profiles), "--binary-path", str(target / "debug/deps"),
            "-s", str(root), "-t", "lcov", "--branch", "--ignore-not-existing", "--ignore", "/*",
            "--ignore", "*/target/*", "--ignore", "*/.cargo/*", "-o", str(lcov)], root, env, args.output, target))
        if stages[-1]["exit_code"] == 0:
            artifact["coverage"] = {"status": "COLLECTED", "totals": coverage_totals(lcov), "path": str(lcov)}
        else:
            artifact["coverage"]["status"] = "FAILED"
    artifact["images_after"] = images(root)


def main():
    args = arguments()
    args.output = args.output.resolve()
    root = pathlib.Path(text(["git", "rev-parse", "--show-toplevel"], pathlib.Path.cwd()))
    started = time.monotonic()
    artifact = {"schema_version": 1, "kind": "PACKAGE_BASELINE", "stages": [],
        "started_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "run": {"id": os.environ.get("GITHUB_RUN_ID"), "attempt": os.environ.get("GITHUB_RUN_ATTEMPT"),
                "job": os.environ.get("GITHUB_JOB"), "job_queued_at": args.job_queued_at, "job_started_at": args.job_started_at},
        "observations": None, "error": None}
    try:
        if args.repeat < 1:
            raise ValueError("Repeat must be positive")
        if args.job_metadata:
            job = json.loads(args.job_metadata.read_text())
            args.job_queued_at, args.job_started_at = job.get("created_at"), job.get("started_at")
            artifact["run"].update(job_id=job.get("id"), job_queued_at=args.job_queued_at, job_started_at=args.job_started_at)
            # Some GitHub API versions omit job creation time. Do not substitute
            # workflow creation time, which would include upstream dependencies.
            if not args.job_queued_at:
                args.job_started_at = None
        if bool(args.job_queued_at) != bool(args.job_started_at):
            raise ValueError("Both job timestamps are required to measure scheduling delay")
        artifact["run"]["scheduling_delay_seconds"] = (
            datetime.datetime.fromisoformat(args.job_started_at.replace("Z", "+00:00")) -
            datetime.datetime.fromisoformat(args.job_queued_at.replace("Z", "+00:00"))).total_seconds() if args.job_queued_at else None
        if (artifact["run"]["scheduling_delay_seconds"] or 0) < 0:
            raise ValueError("Job start precedes queue timestamp")
        benchmark(args, root, artifact)
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        artifact["error"] = str(error)
    artifact["measurement_seconds"] = round(time.monotonic() - started, 6)
    artifact["workflow_seconds"] = None
    artifact["workflow_duration_note"] = "Collect completed workflow/job API timestamps separately; local process duration excludes scheduling and artifact upload"
    artifact["status"] = "FAILED" if artifact["error"] or any(s["exit_code"] for s in artifact["stages"]) else "PASSED"
    # Never replace existing evidence when argument validation fails.
    if args.output.exists() and (args.output / "benchmark.json").exists():
        print(artifact["error"] or "Artifact already exists", file=sys.stderr)
        return 1
    args.output.mkdir(parents=True, exist_ok=True)
    (args.output / "benchmark.json").write_text(json.dumps(artifact, indent=2, sort_keys=True) + "\n")
    print(json.dumps({"status": artifact["status"], "artifact": str(args.output / "benchmark.json"), "error": artifact["error"]}))
    return int(artifact["status"] == "FAILED")


if __name__ == "__main__":
    sys.exit(main())
