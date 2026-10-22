#!/usr/bin/env python3
"""Require the exact lane reports and union their normalized source records."""

import argparse
import pathlib
import re
import sys

from ci_benchmark.lcov import coverage_totals
from ci_coverage import combine, parse_lcov, render


def merge(directory, output, lanes, root=None):
    root = (root or pathlib.Path.cwd()).resolve()
    if not lanes or len(lanes) != len(set(lanes)) or any(not re.fullmatch(r"api|crawler|integration|remaining|all|legacy-[a-z0-9][a-z0-9_-]*", lane) for lane in lanes):
        raise ValueError("Expected distinct, known coverage lanes")
    expected = {directory / f"coverage-{lane}" / f"{lane}.lcov" for lane in lanes}
    actual = set(directory.rglob("*.lcov"))
    if actual != expected:
        raise ValueError(f"Coverage artifact set differs: missing={sorted(map(str, expected - actual))}, unexpected={sorted(map(str, actual - expected))}")
    sources = {}
    for report in sorted(expected):
        coverage_totals(report)
        combine(sources, parse_lcov(report.read_text(), root))
    output.write_text(render(sources))


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
