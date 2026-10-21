#!/usr/bin/env python3
"""Validate package ownership and run each lane's explicit phases sequentially."""

import argparse
import json
import os
import pathlib
import shlex
import subprocess
import sys

from ci_inventory.catalog import LANES, build_catalog, legacy_package_selection
from ci_inventory.discovery import attach_suites
from ci_inventory.io import command, write_json
from ci_inventory.process import execute
from ci_inventory.phases import (PRISTINE_CASE, PRISTINE_TARGET, cargo_args, combined_selection,
                                 libtest_commands, nextest_command, suite_identity,
                                 unmanaged_commands, unmanaged_environment, validate_special_cases)


def arguments():
    parser = argparse.ArgumentParser(description=__doc__)
    selection = parser.add_mutually_exclusive_group(required=True)
    selection.add_argument("--lane", choices=LANES, action="append")
    selection.add_argument("--legacy-package", help="Opt-in comparison only: one original per-package libtest invocation")
    parser.add_argument("--harness", choices=["nextest", "libtest"], required=True)
    parser.add_argument("--slots", type=int, required=True)
    parser.add_argument("--profile", choices=["none", "postgres", "aws", "opensearch", "api"], required=True)
    parser.add_argument("--supervisor", type=pathlib.Path, required=True)
    parser.add_argument("--state-dir", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    parser.add_argument("--features", action="append", default=[])
    parser.add_argument("--manifest", type=pathlib.Path, default=pathlib.Path("ci/test-lanes.json"))
    parser.add_argument("--list-only", action="store_true", help="Compile and verify cases without starting services or test bodies")
    parser.add_argument("--phase", choices=["all", "managed"], default="all", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if not 1 <= args.slots <= 1024:
        parser.error("--slots must be between 1 and 1024")
    if args.legacy_package is not None and (args.profile != "none" or args.harness != "libtest"):
        parser.error("--legacy-package requires libtest and profile none to retain per-package lazy fixtures")
    if args.legacy_package is None and args.profile == "none" and any(lane != "REMAINING" for lane in args.lane):
        parser.error("a managed profile is required for infrastructure, API and crawler lanes")
    args.output = args.output.resolve()
    args.state_dir = args.state_dir.resolve()
    args.supervisor = args.supervisor.resolve()
    return args


def discover_lane(args, root):
    metadata = json.loads(command(["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"], root))
    catalog, expected = build_catalog(metadata, root)
    manifest = json.loads((root / args.manifest).read_text())
    selection = (legacy_package_selection(manifest, expected, args.legacy_package) if args.legacy_package is not None
                 else combined_selection(manifest, expected, args.lane))
    write_json(args.output / "selection.json", selection)
    catalog["packages"] = [package for package in catalog["packages"] if package["name"] in selection["packages"]]
    if any(not target["harness"] for package in catalog["packages"] for target in package["targets"] if target["test"]):
        raise ValueError("Custom harness needs an explicit adapter before lane execution")
    argv = ["cargo", "nextest", "list", *cargo_args(selection, args.features),
            "--all-targets", "--message-format", "json"]
    print(f"test lane discovery: {shlex.join(argv)}", flush=True)
    with (args.output / "compile.log").open("w") as log:
        process = subprocess.run(argv, cwd=root, text=True, stdout=subprocess.PIPE, stderr=log)
    if process.returncode:
        raise RuntimeError(f"Nextest discovery failed ({process.returncode}); see {args.output / 'compile.log'}")
    data = json.loads(process.stdout)
    write_json(args.output / "nextest.json", data)
    attach_suites(catalog, data["rust-suites"], root, args.output)
    suites = {}
    for suite in data["rust-suites"].values():
        identity = suite_identity(suite)
        if identity in suites or identity not in selection["targets"]:
            raise ValueError(f"Duplicate or foreign lane suite: {identity}")
        suites[identity] = suite
    validate_special_cases(selection, suites)
    print(f"test lane discovery: verified {len(suites)} targets, {data['test-count']} cases", flush=True)
    return selection, suites


def supervised(args, profile, slots, phase, argv):
    return [str(args.supervisor), "run", "--profile", profile, "--slots", str(slots),
            "--state-dir", str(args.state_dir / phase), "--", *argv]


def managed_phase(args, root, selection, suites):
    environment = dict(os.environ)
    if args.harness == "nextest":
        execute(nextest_command(selection, args.slots, args.features), root, environment)
    else:
        rust_libdir = command(["rustc", "--print", "target-libdir"], root).strip()
        for identity, suite in sorted(suites.items()):
            local = dict(environment, LD_LIBRARY_PATH=os.pathsep.join([
                str(pathlib.Path(suite["binary-path"]).parent), rust_libdir, environment.get("LD_LIBRARY_PATH", "")]))
            for argv in libtest_commands(identity, suite, args.slots):
                execute([*argv, *(["--nocapture"] if args.legacy_package else [])], suite["cwd"], local)
    if selection["doctest_targets"]:
        execute(["cargo", "test", *cargo_args(selection, args.features), "--doc",
                 *(["--", "--nocapture"] if args.legacy_package else [])], root, environment)


def main():
    args = arguments()
    root = pathlib.Path(command(["git", "rev-parse", "--show-toplevel"]).strip())
    if args.phase == "managed":
        selection = json.loads((args.output / "selection.json").read_text())
        data = json.loads((args.output / "nextest.json").read_text())
        managed_phase(args, root, selection, {suite_identity(suite): suite for suite in data["rust-suites"].values()})
        return
    args.output.mkdir(parents=True, exist_ok=True)
    selection, suites = discover_lane(args, root)
    if args.list_only:
        return
    args.state_dir.mkdir(parents=True, exist_ok=True)
    environment = unmanaged_environment(os.environ)
    if PRISTINE_TARGET in suites:
        pristine = dict(selection, package_args=["--package", "database-migration-lambda"])
        argv = ["cargo", "nextest", "run", *cargo_args(pristine, args.features), "--test-threads", "1",
                "--retries", "0", "--no-tests", "fail", "-E", f"binary(=database-migration-lambda) and test(={PRISTINE_CASE})"]
        execute(supervised(args, "postgres", 1, "pristine", argv), root, environment)
    child = [sys.executable, str(pathlib.Path(__file__).resolve()), *sys.argv[1:], "--phase", "managed"]
    if args.profile == "none":
        execute(child, root, environment)
    else:
        execute(supervised(args, args.profile, args.slots, "managed", child), root, environment)
    for argv in unmanaged_commands(selection, args.features):
        execute(argv, root, environment)


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, ValueError, OSError, KeyError) as error:
        print(f"test lane: {error}", file=sys.stderr)
        sys.exit(1)
