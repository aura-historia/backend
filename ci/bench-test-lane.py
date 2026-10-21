#!/usr/bin/env python3
"""Measure an opt-in lane invocation with the existing stage/RSS/disk collector."""

import argparse
import datetime
import json
import os
import pathlib
import subprocess
import sys
import time

from ci_benchmark.execution import environment_metadata, images, measure
from ci_benchmark.metrics import coverage_totals, observations
from ci_inventory.catalog import LANES


def arguments():
    parser = argparse.ArgumentParser(description=__doc__)
    selection = parser.add_mutually_exclusive_group(required=True)
    selection.add_argument("--lane", choices=LANES, action="append")
    selection.add_argument("--legacy-package", help="Compare original package scheduling with libtest and lazy fixtures")
    parser.add_argument("--harness", choices=["nextest", "libtest"], required=True)
    parser.add_argument("--slots", type=int, required=True)
    parser.add_argument("--profile", choices=["none", "postgres", "aws", "opensearch", "api"], required=True)
    parser.add_argument("--features", action="append", default=[])
    parser.add_argument("--cache", choices=["COLD_TARGET", "WARM_TARGET"], required=True)
    parser.add_argument("--target-dir", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    return parser.parse_args()


def commands(args, root, target, output):
    selection = (["--legacy-package", args.legacy_package] if args.legacy_package is not None else
                 [value for lane in args.lane for value in ("--lane", lane)])
    driver = [sys.executable, str(root / "ci/run-test-lane.py"), *selection,
              "--harness", args.harness, "--slots", str(args.slots), "--profile", args.profile,
              *[value for feature in args.features for value in ("--features", feature)],
              "--supervisor", str(target / "debug/aura-test-env"),
              "--state-dir", str(output / "state"), "--output", str(output / "discovery")]
    compile = list(driver)
    compile[compile.index("--output") + 1] = str(output / "compile-discovery")
    result = [("COMPILE_LINK", [*compile, "--list-only"])]
    artifacts = [output / "compile-discovery/compile.jsonl", output / "discovery/compile.jsonl"]
    if args.profile != "none" or args.legacy_package in {"database-migration-lambda", "test-api"}:
        artifacts.append(output / "supervisor.jsonl")
        result.append(("SUPERVISOR_COMPILE", ["bash", "-c",
            'cargo build --locked --package test-api --all-features --bin aura-test-env --message-format=json-render-diagnostics > "$1"',
            "_", str(artifacts[-1])]))
    return [*result, ("TESTS_AND_FIXTURES", driver), ("COVERAGE", [sys.executable, str(root / "ci/export-coverage.py"),
            "--profiles", str(output / "profiles"), "--nextest", str(output / "discovery/nextest.json"),
            "--doctests", str(output / "doctests"), *[value for artifact in artifacts for value in ("--cargo-artifacts", str(artifact))],
            "--output", str(output / "coverage.lcov")])]


def measurement_environment(root, target, output):
    return dict(os.environ, CARGO_TARGET_DIR=str(target), CARGO_INCREMENTAL="0", CARGO_PROFILE_TEST_DEBUG="1", CARGO_PROFILE_DEV_DEBUG="1", CARGO_TERM_COLOR="never",
        RUSTUP_TOOLCHAIN="nightly-2026-10-20", RUSTFLAGS="-C instrument-coverage -Zcoverage-options=branch",
        RUSTDOCFLAGS="-C instrument-coverage -Zcoverage-options=branch", RUSTDOC=str(root / "ci/coverage-rustdoc.py"),
        AURA_CI_RUSTDOC=subprocess.check_output(["rustup", "which", "--toolchain", "nightly-2026-10-20", "rustdoc"], text=True).strip(),
        AURA_CI_COVERAGE_DIR=str(output), LLVM_PROFILE_FILE=str(output / "profiles/discovery/%b-%4m.profraw"), NEXTEST_PROFILE="ci",
        AURA_CI_BENCHMARK_TIMING="1", NEXTEST_SUCCESS_OUTPUT="final", AWS_ACCESS_KEY_ID="test", AWS_SECRET_ACCESS_KEY="test", AWS_SESSION_TOKEN="test",
        AWS_EC2_METADATA_DISABLED="true", AWS_DEFAULT_REGION="eu-central-1", AWS_REGION="eu-central-1")


def main():
    args = arguments()
    root = pathlib.Path(subprocess.check_output(["git", "rev-parse", "--show-toplevel"], text=True).strip())
    target, output = args.target_dir.resolve(), args.output.resolve()
    target.mkdir(parents=True, exist_ok=True)
    if any(target.iterdir()) != (args.cache == "WARM_TARGET"):
        raise ValueError("COLD_TARGET needs an empty target; WARM_TARGET needs a previously built target")
    output.mkdir(parents=True, exist_ok=False)
    (output / "profiles/discovery").mkdir(parents=True)
    environment = measurement_environment(root, target, output)
    started = time.monotonic()
    report = {"kind": "FIXED_LANE_COMPARISON", "revision": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
        "dirty": bool(subprocess.check_output(["git", "status", "--porcelain"], text=True).strip()),
        "started_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "lanes": args.lane, "legacy_package": args.legacy_package, "harness": args.harness, "slots": args.slots, "profile": args.profile, "features": args.features,
        "cache": args.cache, "runner": environment_metadata(root, environment), "images_before": images(root),
        "run": {key: os.environ.get(key) for key in ("GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT", "GITHUB_JOB", "GITHUB_WORKFLOW_REF", "GITHUB_WORKFLOW_SHA")},
        "stages": [], "status": "FAILED", "error": None}
    try:
        for name, argv in commands(args, root, target, output):
            guarded = [sys.executable, str(root / 'ci/guard-disk.py'), '--target', str(target),
                       '--profiles', str(output / 'profiles'), '--log', str(output / 'disk.jsonl'), '--', *argv]
            report["stages"].append(measure(name, guarded, root, environment, output, target))
            if report["stages"][-1]["exit_code"]:
                break
        if all(stage["exit_code"] == 0 for stage in report["stages"]):
            report["coverage"] = coverage_totals(output / "coverage.lcov")
            report["observations"] = observations(output / "tests_and_fixtures.log")
            report["status"] = "PASSED"
    except (OSError, ValueError, RuntimeError) as error:
        report["error"] = str(error)
    finally:
        report["measurement_seconds"] = time.monotonic() - started
        report["images_after"] = images(root)
        (output / "benchmark.json").write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
    print(json.dumps({"status": report["status"], "output": str(output)}))
    return int(report["status"] != "PASSED")


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(f"lane measurement: {error}", file=sys.stderr)
        sys.exit(1)
