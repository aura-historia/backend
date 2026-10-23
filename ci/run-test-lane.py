#!/usr/bin/env python3
"""Run owned Nextest targets, the pristine migration case and explicit doctests."""

import argparse
import json
import os
import pathlib
import sys

from ci_inventory.catalog import LANES, lane_selection, workspace_packages
from ci_inventory.discovery import validate_discovered_supervisor, validate_suites
from ci_inventory.io import command, write_json
from ci_inventory.process import execute
from ci_inventory.phases import (PRISTINE_CASE, PRISTINE_FILTER, PRISTINE_TARGET,
                                 cargo_args, nextest_command, standalone_environment)


def arguments():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lane", choices=LANES, action="append", required=True)
    parser.add_argument("--slots", type=int, required=True)
    parser.add_argument("--profile", choices=["none", "postgres", "aws", "opensearch", "api"], required=True)
    parser.add_argument("--supervisor", type=pathlib.Path, required=True)
    parser.add_argument("--state-dir", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    parser.add_argument("--features", action="append", default=[])
    parser.add_argument("--manifest", type=pathlib.Path, default=pathlib.Path("ci/test-lanes.json"))
    parser.add_argument("--list-only", action="store_true", help="Compile and validate owned suites without services or tests")
    parser.add_argument("--phase", choices=["all", "managed"], default="all", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if not 1 <= args.slots <= 1024:
        parser.error("--slots must be between 1 and 1024")
    if args.profile == "none" and any(lane != "REMAINING" for lane in args.lane):
        parser.error("a managed profile is required for infrastructure, API and crawler lanes")
    args.output = args.output.resolve()
    args.state_dir = args.state_dir.resolve()
    args.supervisor = args.supervisor.resolve()
    return args


def discover_lane(args, root):
    packages = workspace_packages(root)
    manifest = json.loads((root / args.manifest).read_text())
    selection = lane_selection(manifest, packages, args.lane)
    write_json(args.output / "selection.json", selection)
    argv = ["cargo", "nextest", "list", *cargo_args(selection, args.features),
            "--all-targets", "--message-format", "json"]
    with (args.output / "nextest.json").open("w") as output:
        execute(argv, root, os.environ, stdout=output)
    data = json.loads((args.output / "nextest.json").read_text())
    suites = validate_suites(data, packages, selection)
    if "database-migration-lambda" in selection["packages"]:
        case = suites[PRISTINE_TARGET]["testcases"].get(PRISTINE_CASE)
        if case is None or case["ignored"]:
            raise ValueError(f"Required pristine migration case is absent or ignored: {PRISTINE_CASE}")
    if "test-api" in selection["packages"]:
        validate_discovered_supervisor(data, args.supervisor)
    print(f"Verified {len(suites)} owned targets, {data['test-count']} cases", flush=True)
    return selection, suites


def supervised(args, profile, slots, phase, argv):
    return [str(args.supervisor), "run", "--profile", profile, "--slots", str(slots),
            "--state-dir", str(args.state_dir / phase), "--", *argv]


def managed_phase(args, root):
    selection = json.loads((args.output / "selection.json").read_text())
    execute(nextest_command(selection, args.slots, args.features), root, os.environ,
            junit=args.output / "managed-junit.xml")
    if selection["doctest_targets"]:
        execute(["cargo", "test", *cargo_args(selection, args.features), "--doc"], root, os.environ)


def main():
    args = arguments()
    root = pathlib.Path(command(["git", "rev-parse", "--show-toplevel"]).strip())
    if args.phase == "managed":
        managed_phase(args, root)
        return
    args.output.mkdir(parents=True, exist_ok=True)
    selection, suites = discover_lane(args, root)
    if args.list_only:
        return
    args.state_dir.mkdir(parents=True, exist_ok=True)
    environment = standalone_environment(os.environ)
    if PRISTINE_TARGET in suites:
        pristine = dict(selection, package_args=["--package", "database-migration-lambda"])
        argv = ["cargo", "nextest", "run", *cargo_args(pristine, args.features), "--test-threads", "1",
                "--retries", "0", "--no-tests", "fail", "-E", PRISTINE_FILTER]
        execute(supervised(args, "postgres", 1, "pristine", argv), root, environment,
                junit=args.output / "pristine-junit.xml")
    child = [sys.executable, str(pathlib.Path(__file__).resolve()), *sys.argv[1:], "--phase", "managed"]
    if args.profile != "none":
        child = supervised(args, args.profile, args.slots, "managed", child)
    execute(child, root, environment)


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, ValueError, OSError, KeyError) as error:
        print(f"test lane: {error}", file=sys.stderr)
        sys.exit(1)
