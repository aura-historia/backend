#!/usr/bin/env python3
"""Validate the exact required lane reports before the existing Sonar concatenation."""

import argparse
import pathlib
import sys

from ci_benchmark.lcov import coverage_totals


def merge(directory, output, lanes):
    if not lanes or len(lanes) != len(set(lanes)) or any(lane not in {"api", "crawler", "integration", "remaining", "all"} for lane in lanes):
        raise ValueError("Expected distinct, known coverage lanes")
    expected = {directory / f"coverage-{lane}" / f"{lane}.lcov" for lane in lanes}
    actual = set(directory.rglob("*.lcov"))
    if actual != expected:
        raise ValueError(f"Coverage artifact set differs: missing={sorted(map(str, expected - actual))}, unexpected={sorted(map(str, actual - expected))}")
    for report in sorted(expected):
        coverage_totals(report)
    output.write_text("".join(report.read_text().rstrip("\n") + "\n" for report in sorted(expected)))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    parser.add_argument("lanes", nargs="+")
    args = parser.parse_args()
    try:
        merge(args.input, args.output, args.lanes)
    except (OSError, ValueError) as error:
        print(f"coverage: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
