#!/usr/bin/env python3
"""Validate complete Cargo workspace ownership and emit current lane selections."""

import argparse
import json
import pathlib
import sys

from ci_inventory.catalog import LANES, check_manifest, lane_selection, workspace_packages
from ci_inventory.io import command, write_json


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    parser.add_argument("--manifest", type=pathlib.Path, default=pathlib.Path("ci/test-lanes.json"))
    parser.add_argument("--lane", choices=LANES, help="Write selected Cargo arguments to lane-selection.json")
    args = parser.parse_args()
    root = pathlib.Path(command(["git", "rev-parse", "--show-toplevel"]).strip())
    packages = workspace_packages(root)
    manifest = json.loads((root / args.manifest).read_text())
    check_manifest(manifest, packages)
    write_json(args.output / "inventory.json", lane_selection(manifest, packages, LANES))
    if args.lane:
        write_json(args.output / "lane-selection.json", lane_selection(manifest, packages, [args.lane]))
    print(f"Validated {len(packages)} workspace packages across four lanes")


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, ValueError, OSError, KeyError) as error:
        print(f"inventory: {error}", file=sys.stderr)
        sys.exit(1)
