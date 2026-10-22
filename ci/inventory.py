#!/usr/bin/env python3
"""Inventory every Cargo test target without selecting or running test bodies."""

import argparse
import json
import pathlib
import sys

from ci_inventory.catalog import LANES, build_catalog, check_manifest, feature_inventory, lane_selection
from ci_inventory.discovery import discover
from ci_inventory.io import command, write_json


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    parser.add_argument("--manifest", type=pathlib.Path, default=pathlib.Path("ci/test-lanes.json"))
    parser.add_argument("--write-manifest", action="store_true", help="Explicitly accept the live package/target set")
    parser.add_argument("--lane", choices=LANES, help="Write validated package selectors to lane-selection.json")
    parser.add_argument("--features", action="store_true", help="Compare workspace and every package's Cargo feature graph")
    parser.add_argument("--discover", action="store_true", help="Compile/list nextest, libtest and doctest cases; run no test bodies")
    args = parser.parse_args()
    root = pathlib.Path(command(["git", "rev-parse", "--show-toplevel"]).strip())
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    metadata = json.loads(command(["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"], root))
    catalog, expected = build_catalog(metadata, root)
    if args.write_manifest:
        write_json(root / args.manifest, expected)
    manifest = json.loads((root / args.manifest).read_text())
    check_manifest(manifest, expected)
    if args.lane:
        write_json(args.output / "lane-selection.json", lane_selection(manifest, expected, args.lane))
    catalog["revision"] = command(["git", "rev-parse", "HEAD"], root).strip()
    catalog["dirty"] = bool(command(["git", "status", "--porcelain"], root).strip())
    catalog["toolchain"] = command(["rustc", "-vV"], root).strip()
    if args.features:
        catalog["feature_comparison"] = feature_inventory(catalog["packages"], root, args.output)
    if args.discover:
        discover(catalog, root, args.output)
    write_json(args.output / "inventory.json", catalog)
    print(json.dumps(catalog["summary"], sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, ValueError, OSError) as error:
        print(f"inventory: {error}", file=sys.stderr)
        sys.exit(1)
