#!/usr/bin/env python3
"""Export lane coverage with the selected Rust toolchain's bundled LLVM tools."""

import argparse
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import time

from ci_coverage import apply_branch_regions, parse_lcov, render, source_path
from ci_inventory.io import command, write_json


def run_llvm(argv, output=None):
    started = time.monotonic()
    try:
        subprocess.run(argv, stdout=output, check=True)
    finally:
        format = next((argument for argument in argv if argument.startswith("--format=")), "")
        print(f"coverage {pathlib.Path(argv[0]).name} {format}: {time.monotonic() - started:.3f}s", flush=True)


def coverage_objects(nextest, cargo_artifacts, doctests, root):
    if not cargo_artifacts:
        raise ValueError("No Cargo coverage artifact metadata was recorded")
    data = json.loads(nextest.read_text())
    meta = data["rust-build-meta"]
    objects = {pathlib.Path(suite["binary-path"]).resolve() for suite in data["rust-suites"].values()}
    for binaries in meta["non-test-binaries"].values():
        for binary in binaries:
            objects.add((pathlib.Path(meta["target-directory"]) / binary["path"]).resolve())
    for artifact in cargo_artifacts:
        messages = [json.loads(line) for line in artifact.read_text().splitlines()]
        finished = [item for item in messages if item.get("reason") == "build-finished"]
        if (len(finished) != 1 or finished[0].get("success") is not True
                or not any(item.get("reason") == "compiler-artifact" for item in messages)):
            raise ValueError(f"Cargo coverage artifact metadata is incomplete: {artifact}")
        for item in messages:
            if item.get("reason") != "compiler-artifact":
                continue
            if item.get("executable"):
                objects.add(pathlib.Path(item["executable"]).resolve())
            elif "proc-macro" in item.get("target", {}).get("kind", []) and source_path(item["target"]["src_path"], root):
                libraries = [pathlib.Path(filename).resolve() for filename in item["filenames"] if filename.endswith(".so")]
                if not libraries:
                    raise ValueError("Compiled repository proc-macro has no recorded shared object")
                objects.update(libraries)
    for path in doctests.rglob("*"):
        if path.is_file() and os.access(path, os.X_OK):
            with path.open("rb") as stream:
                if stream.read(4) == b"\x7fELF":
                    objects.add(path.resolve())
    if not objects:
        raise ValueError("No compiled coverage objects were recorded")
    for path in objects:
        if not path.is_file():
            raise ValueError(f"Compiled coverage object is missing: {path}")
    return sorted(objects)


def coverage_profiles(profiles):
    raw = sorted(profiles.rglob("*.profraw"))
    execution = [path for path in raw if "discovery" not in path.relative_to(profiles).parts]
    if not execution or any(not path.stat().st_size for path in raw):
        raise ValueError("No nonempty execution coverage profiles were written")
    return raw, execution


def export(profiles, nextest, cargo_artifacts, doctests, output, root):
    objects = coverage_objects(nextest, cargo_artifacts, doctests, root)
    raw, execution = coverage_profiles(profiles)
    sysroot = pathlib.Path(command(["rustc", "--print", "sysroot"]).strip())
    host = next(line.split(": ", 1)[1] for line in command(["rustc", "-Vv"]).splitlines() if line.startswith("host: "))
    llvm = sysroot / "lib" / "rustlib" / host / "bin"
    output.parent.mkdir(parents=True, exist_ok=True)
    write_json(output.with_suffix(".objects.json"), {"objects": list(map(str, objects)), "profiles": list(map(str, raw)),
               "execution_profiles": list(map(str, execution)), "cargo_artifacts": list(map(str, cargo_artifacts)),
               "rustc": command(["rustc", "-Vv"]).strip(), "llvm_cov": command([str(llvm / "llvm-cov"), "--version"]).strip()})
    with tempfile.TemporaryDirectory(prefix="llvm-coverage-", dir=output.parent) as temporary:
        directory = pathlib.Path(temporary)
        inputs = directory / "profiles.txt"
        inputs.write_text("\n".join(map(str, raw)) + "\n")
        run_llvm([str(llvm / "llvm-profdata"), "merge", "-sparse", f"--input-files={inputs}",
                  "-o", str(directory / "merged.profdata")])
        argv = [str(llvm / "llvm-cov"), "export", str(objects[0]), *[f"--object={path}" for path in objects[1:]],
                f"--instr-profile={directory / 'merged.profdata'}"]
        for format, suffix in (("lcov", "lcov"), ("text", "json")):
            with (directory / suffix).open("w") as stream:
                run_llvm([*argv, f"--format={format}"], stream)
        sources = parse_lcov((directory / "lcov").read_text(), root)
        apply_branch_regions(sources, json.loads((directory / "json").read_text()), root)
        result = directory / "normalized.lcov"
        result.write_text(render(sources))
        result.replace(output)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profiles", type=pathlib.Path, required=True)
    parser.add_argument("--nextest", type=pathlib.Path, required=True)
    parser.add_argument("--cargo-artifacts", type=pathlib.Path, action="append", required=True)
    parser.add_argument("--doctests", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    parser.add_argument("--root", type=pathlib.Path, default=pathlib.Path.cwd())
    args = parser.parse_args()
    started = time.monotonic()
    try:
        export(args.profiles, args.nextest, args.cargo_artifacts, args.doctests, args.output, args.root.resolve())
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        print(f"coverage: {error}", file=sys.stderr)
        return 1
    finally:
        print(f"coverage total: {time.monotonic() - started:.3f}s", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
