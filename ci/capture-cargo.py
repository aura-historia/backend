#!/usr/bin/env python3
"""Retain exact Cargo artifact metadata and mutable executables for coverage."""

import argparse
import os
import pathlib
import sys

from ci_inventory.coverage import execute_cargo


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    argv = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not argv:
        parser.error("a Cargo command is required")
    try:
        execute_cargo(argv, pathlib.Path.cwd(), os.environ, args.output.resolve())
    except (OSError, ValueError) as error:
        print(f"Cargo coverage capture: {error}", file=sys.stderr)
        sys.exit(1)
