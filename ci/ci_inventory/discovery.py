"""Compiled nextest/libtest parity and separate rustdoc discovery."""

import json
import os
import pathlib
import re
import subprocess

from .io import command, write_json


def nextest_data(raw):
    records = [json.loads(line) for line in raw.splitlines()]
    reports = [record for record in records if "rust-suites" in record]
    if len(reports) != 1 or any("reason" not in record and "rust-suites" not in record for record in records):
        raise ValueError("Discovery must contain Cargo messages and exactly one Nextest report")
    return reports[0]


def listed_cases(raw):
    return {match[1] for line in raw.splitlines() if (match := re.fullmatch(r"(.+): (?:test|benchmark)", line))}


def list_harness(suite, output, rust_libdir):
    path = pathlib.Path(suite["binary-path"])
    if not path.is_file():
        raise ValueError(f"Discovered binary is missing: {path}")
    environment = dict(os.environ)
    # Cargo supplies these paths when it runs a harness; direct parity listing
    # needs the same runtime linker search path.
    environment["LD_LIBRARY_PATH"] = os.pathsep.join([str(path.parent), rust_libdir, environment.get("LD_LIBRARY_PATH", "")])
    results = []
    for extra in ([], ["--ignored"]):
        process = subprocess.run([str(path), "--list", "--format", "terse", *extra], cwd=suite["cwd"],
                                 env=environment, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        if process.returncode:
            raise RuntimeError(f"Harness list failed for {suite['binary-id']}: {process.stderr}")
        results.append(process.stdout)
    output.write_text(results[0])
    return listed_cases(results[0]), listed_cases(results[1])


def attach_suites(catalog, suites, root, output):
    packages = {package["name"]: package for package in catalog["packages"]}
    rust_libdir = command(["rustc", "--print", "target-libdir"], root).strip()
    seen = set()
    for identity, suite in suites.items():
        package = packages[suite["package-name"]]
        if pathlib.Path(suite["cwd"]).resolve() != (root / package["path"]).resolve():
            raise ValueError(f"Nextest list belongs to another checkout: {identity}")
        kind = "proc-macro" if suite["kind"] == "proc-macro" else suite["kind"]
        candidates = [target for target in package["targets"] if target["name"] == suite["binary-name"] and kind in target["kind"]]
        if len(candidates) != 1:
            raise ValueError(f"Nextest suite has no unique Cargo target: {identity}")
        target = candidates[0]
        if target["id"] in seen:
            raise ValueError(f"Duplicate suite for Cargo target: {identity}")
        cases, ignored = list_harness(suite, output / (re.sub(r"[^\w.-]", "_", identity) + ".list.txt"), rust_libdir)
        expected = suite["testcases"]
        if cases != set(expected) or ignored != {name for name, case in expected.items() if case["ignored"]}:
            raise ValueError(f"Nextest/libtest case or ignored parity mismatch: {identity}")
        target["discovery"] = {"status": "VERIFIED", "test_count": len(cases), "ignored_count": len(ignored),
                               "cases": [{"name": name, "ignored": name in ignored} for name in sorted(cases)]}
        seen.add(target["id"])
    for package in catalog["packages"]:
        for target in package["targets"]:
            if target["id"] not in seen:
                if target["test"] and target["harness"]:
                    raise ValueError(f"Cargo test target disappeared from nextest: {target['id']}")
                target["discovery"] = {"status": "NOT_A_STANDARD_TEST_HARNESS", "test_count": None, "ignored_count": None}


def discover_doctests(catalog, root, output):
    targets = {}
    for package in catalog["packages"]:
        for target in package["targets"]:
            if target["doctest"]:
                if target["name"] in targets:
                    raise ValueError(f"Ambiguous rustdoc target name requires package-specific discovery: {target['name']}")
                targets[target["name"]] = target
    for ignored_only in (False, True):
        seen = set()
        argv = ["cargo", "test", "--locked", "--workspace", "--all-features", "--doc", "--", "--list", "--format", "terse"]
        if ignored_only:
            argv.append("--ignored")
        process = subprocess.run(argv, cwd=root, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        raw = process.stdout
        (output / ("doctests-ignored.log" if ignored_only else "doctests.log")).write_text(raw)
        if process.returncode:
            raise RuntimeError(f"Rustdoc listing failed ({process.returncode}): {raw[-4000:]}")
        current = None
        for line in raw.splitlines():
            heading = re.fullmatch(r"\s*Doc-tests (\S+)\s*", line)
            if heading:
                if heading[1] not in targets or heading[1] in seen:
                    raise ValueError(f"Unknown or repeated doctest target heading: {heading[1]}")
                current = targets[heading[1]]
                seen.add(heading[1])
                if not ignored_only:
                    current["doctest_discovery"] = {"status": "LISTED", "feature_scope": "WORKSPACE_ALL_FEATURES", "cases": [], "ignored_cases": []}
            elif listed_cases(line):
                if current is None:
                    raise ValueError("Doctest case has no target heading")
                current["doctest_discovery"]["ignored_cases" if ignored_only else "cases"].extend(sorted(listed_cases(line)))
        if seen != set(targets):
            raise ValueError("Doctest targets disappeared from " + ("ignored" if ignored_only else "complete") +
                             " listing: " + ", ".join(sorted(set(targets) - seen)))
    for target in targets.values():
        discovery = target["doctest_discovery"]
        discovery["test_count"] = len(discovery["cases"])
        discovery["ignored_count"] = len(discovery["ignored_cases"])
        if not set(discovery["ignored_cases"]) <= set(discovery["cases"]):
            raise ValueError(f"Ignored doctests absent from complete list: {target['id']}")


def discover(catalog, root, output):
    directory = output / "discovery"
    directory.mkdir(exist_ok=True)
    # Always refresh the binary list: a list from the same revision can still be
    # stale in a dirty checkout or under different compiler/feature flags.
    raw = command(["cargo", "nextest", "list", "--locked", "--workspace", "--all-features",
                   "--all-targets", "--message-format", "json"], root)
    data = json.loads(raw)
    write_json(directory / "nextest.json", data)
    attach_suites(catalog, data["rust-suites"], root, directory)
    discover_doctests(catalog, root, directory)
    targets = [target for package in catalog["packages"] for target in package["targets"]]
    catalog["summary"]["test_count"] = sum(target["discovery"]["test_count"] or 0 for target in targets)
    catalog["summary"]["ignored_count"] = sum(target["discovery"]["ignored_count"] or 0 for target in targets)
    catalog["summary"]["doctest_count"] = sum(target["doctest_discovery"]["test_count"] or 0 for target in targets)
    if catalog["summary"]["test_count"] != data["test-count"]:
        raise ValueError("Nextest total does not match the target inventory")
    catalog["discovery"] = {"status": "VERIFIED", "nextest": command(["cargo", "nextest", "--version"], root).strip(),
                            "unit_integration_features": "WORKSPACE_ALL_FEATURES",
                            "doctest_features": "WORKSPACE_ALL_FEATURES",
                            "parity": "NEXTEST_EQUALS_EACH_LIBTEST_HARNESS_INCLUDING_IGNORED",
                            "platform": data["rust-build-meta"].get("platforms", {})}
