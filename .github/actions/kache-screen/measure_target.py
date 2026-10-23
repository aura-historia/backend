#!/usr/bin/env python3
"""Measure what a workspace compiler-output cache would have to store for one lane."""

import argparse
import json
import os
import pathlib
import subprocess
import tempfile
import time


def workspace_targets(root):
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"], cwd=root))
    targets = [target for package in metadata["packages"] for target in package["targets"]]
    names = {target["name"].replace("-", "_") for target in targets}
    libraries = {target["name"].replace("-", "_") for target in targets
                 if {"lib", "rlib", "proc-macro"} & set(target["kind"])}
    packages = {package["name"] for package in metadata["packages"]}
    return names, libraries, packages


def crate_of(path):
    stem = path.name.split(".", 1)[0]
    if path.suffix in (".rlib", ".rmeta", ".so") and stem.startswith("lib"):
        stem = stem[3:]
    return stem.rsplit("-", 1)[0]


def kind_of(path, info):
    if path.suffix in (".rlib", ".rmeta", ".so"):
        return "library"
    if path.suffix == ".d":
        return "dep-info"
    if path.name.startswith(("build_script_", "build-script-")):
        return "build-script"
    if not path.suffix and info.st_mode & 0o111:
        return "executable"
    return "other"


def owner_of(relative, path, names, libraries, packages):
    """Support Cargo's deps/ layout and the build/<package>/<hash>/ layout."""
    top = relative.parts[0]
    if top == "build" and len(relative.parts) >= 3:
        package = relative.parts[1]
        if package not in packages and "-" in package:
            package = package.rsplit("-", 1)[0]
        return "workspace" if package in packages else "external"
    if top == "deps":
        owned = libraries if path.suffix in (".rlib", ".rmeta", ".so") else names
        return "workspace" if crate_of(path) in owned else "external"
    return "cargo"


def classify(target, names, libraries, packages):
    categories, seen = {}, set()
    debug = target / "debug"
    for path in debug.rglob("*"):
        if not path.is_file() or path.is_symlink():
            continue
        relative = path.relative_to(debug)
        if relative.parts[0] in (".fingerprint", "incremental") or len(relative.parts) == 1:
            continue
        info = path.stat()
        if (info.st_dev, info.st_ino) in seen:
            continue
        seen.add((info.st_dev, info.st_ino))
        name = f"{owner_of(relative, path, names, libraries, packages)}/{kind_of(path, info)}"
        entry = categories.setdefault(name, {"files": [], "bytes": 0})
        entry["files"].append(str(path))
        entry["bytes"] += info.st_size
    return categories


def compressed(files):
    started = time.monotonic()
    with tempfile.NamedTemporaryFile("wb") as listing:
        listing.write(b"\0".join(path.encode() for path in files))
        listing.flush()
        archive = subprocess.Popen(["tar", "--posix", "-cf", "-", "--null", "-T", listing.name],
                                   stdout=subprocess.PIPE)
        zstd = subprocess.Popen(["zstd", "-T0", "-3", "-c"], stdin=archive.stdout, stdout=subprocess.PIPE)
        archive.stdout.close()
        size = 0
        while chunk := zstd.stdout.read(1 << 20):
            size += len(chunk)
        if archive.wait() or zstd.wait():
            raise RuntimeError("compression measurement failed")
    return size, time.monotonic() - started


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    args = parser.parse_args()
    root = pathlib.Path.cwd()
    names, libraries, packages = workspace_targets(root)
    categories = classify(args.target, names, libraries, packages)
    report = {}
    for name, entry in sorted(categories.items()):
        report[name] = {"files": len(entry["files"]), "bytes": entry["bytes"]}
        if name.startswith("workspace/") and entry["bytes"]:
            size, seconds = compressed(entry["files"])
            report[name].update(zstd3_bytes=size, zstd3_seconds=round(seconds, 1))
        print(json.dumps({name: report[name]}), flush=True)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")


if __name__ == "__main__":
    main()
