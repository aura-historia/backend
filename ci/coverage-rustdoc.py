#!/usr/bin/env python3
"""Retain doctest executables separately for every Cargo crate."""

import json
import os
import pathlib
import re
import subprocess
import sys


def main():
    arguments = sys.argv[1:]
    rustdoc = os.environ["AURA_CI_RUSTDOC"]
    if "--test" not in arguments:
        os.execv(rustdoc, [rustdoc, *arguments])
    crate = arguments[arguments.index("--crate-name") + 1]
    if not re.fullmatch(r"[A-Za-z0-9_]+", crate):
        raise ValueError("Invalid doctest crate name")
    directory = pathlib.Path(os.environ["AURA_CI_COVERAGE_DIR"]) / "doctests" / crate
    directory.mkdir(parents=True, exist_ok=True)
    result = subprocess.run([rustdoc, *arguments, "-Zunstable-options", "--persist-doctests", str(directory)])
    (directory / "result.json").write_text(json.dumps({"crate": crate, "exit_code": result.returncode}) + "\n")
    return result.returncode if result.returncode >= 0 else 128 - result.returncode


if __name__ == "__main__":
    sys.exit(main())
