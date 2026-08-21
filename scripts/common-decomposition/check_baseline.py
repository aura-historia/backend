#!/usr/bin/env python3
"""Reject growth of the legacy `common` compatibility crate."""

from __future__ import annotations

import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path
from typing import Any, NoReturn

ROOT = Path(__file__).resolve().parents[2]
BASELINE_PATH = ROOT / "scripts/common-decomposition/baseline.json"
COMMON_MANIFEST = ROOT / "src/common/Cargo.toml"
COMMON_LIB = ROOT / "src/common/src/lib.rs"
KIND_NAMES = {None: "normal", "dev": "development", "build": "build"}
INITIAL_BASELINE_SHA256 = "e5acd8656db0ef9eed4f1eaf777cd4077ccdbb5d6e4e8fac0eabf11832431ea9"


def fail(message: str) -> NoReturn:
    print(f"common decomposition baseline failed: {message}", file=sys.stderr)
    raise SystemExit(1)


def read_json(path: Path) -> dict[str, Any]:
    try:
        return json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot read {path.relative_to(ROOT)}: {error}")


def cargo_metadata() -> dict[str, Any]:
    result = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        cwd=ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        fail(f"cargo metadata failed:\n{result.stderr}")
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError as error:
        fail(f"cargo metadata returned invalid JSON: {error}")


def actual_dependencies(metadata: dict[str, Any]) -> dict[str, dict[str, list[str]]]:
    dependencies: dict[str, dict[str, set[str]]] = {
        "normal": {},
        "development": {},
        "build": {},
    }
    for package in metadata["packages"]:
        for dependency in package["dependencies"]:
            if dependency["name"] != "common":
                continue
            kind = KIND_NAMES.get(dependency["kind"])
            if kind is None:
                fail(
                    f"unsupported dependency kind {dependency['kind']!r} for "
                    f"{package['name']}"
                )
            dependencies[kind].setdefault(package["name"], set()).update(
                dependency["features"]
            )
    return {
        kind: {name: sorted(features) for name, features in consumers.items()}
        for kind, consumers in dependencies.items()
    }


def actual_forwarded_common_features(
    metadata: dict[str, Any],
) -> dict[str, dict[str, list[str]]]:
    forwarded: dict[str, dict[str, list[str]]] = {}
    for package in metadata["packages"]:
        aliases = {
            dependency.get("rename") or dependency["name"]
            for dependency in package["dependencies"]
            if dependency["name"] == "common"
        }
        package_features: dict[str, list[str]] = {}
        for feature, values in package["features"].items():
            common_features = sorted(
                value.split("/", maxsplit=1)[1]
                for value in values
                if any(
                    value.startswith(f"{alias}/")
                    or value.startswith(f"{alias}?/")
                    for alias in aliases
                )
            )
            if common_features:
                package_features[feature] = common_features
        if package_features:
            forwarded[package["name"]] = package_features
    return forwarded


def actual_common_features() -> list[str]:
    manifest = COMMON_MANIFEST.read_text()
    feature_section = manifest.split("[features]", maxsplit=1)
    if len(feature_section) != 2:
        fail("src/common/Cargo.toml has no [features] section")
    return sorted(
        match.group(1)
        for match in re.finditer(r"(?m)^([A-Za-z0-9_-]+)\s*=", feature_section[1])
        if match.group(1) != "default"
    )


def actual_public_modules() -> list[str]:
    source = COMMON_LIB.read_text()
    modules = re.findall(
        r"(?m)^\s*pub\s+mod\s+([A-Za-z0-9_]+)\s*(?:;|\{)",
        source,
    )
    public_reexport = ["__public_reexport__"] if re.search(
        r"(?m)^\s*pub\s+use\s+", source
    ) else []
    return sorted(set(modules) | set(public_reexport))


def check_exact(name: str, actual: Any, expected: Any) -> None:
    if actual != expected:
        fail(
            f"{name} differs from the allowlist. Remove a stale allowlist entry "
            f"when deleting an existing use, but do not add entries.\n"
            f"expected: {json.dumps(expected, indent=2, sort_keys=True)}\n"
            f"actual: {json.dumps(actual, indent=2, sort_keys=True)}"
        )


def check_baseline_only_shrinks(baseline: dict[str, Any], base_ref: str) -> None:
    result = subprocess.run(
        ["git", "show", f"{base_ref}:scripts/common-decomposition/baseline.json"],
        cwd=ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        actual_hash = hashlib.sha256(BASELINE_PATH.read_bytes()).hexdigest()
        if actual_hash != INITIAL_BASELINE_SHA256:
            fail("bootstrap baseline must match its pinned initial hash")
        print("common decomposition baseline: pinned initial baseline accepted")
        return
    try:
        previous = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        fail(f"prior baseline is invalid JSON: {error}")

    for key in ("common_features", "common_public_modules"):
        if not set(baseline[key]).issubset(previous[key]):
            fail(f"{key} may only shrink")

    for key in ("direct_dependencies", "forwarded_common_features"):
        for kind, consumers in baseline[key].items():
            previous_consumers = previous[key].get(kind, {})
            if not set(consumers).issubset(previous_consumers):
                fail(f"{key} consumers may only shrink")
            for consumer, features in consumers.items():
                if isinstance(features, list):
                    if not set(features).issubset(previous_consumers[consumer]):
                        fail(f"{key} features for {consumer} may only shrink")
                    continue
                previous_features = previous_consumers[consumer]
                if not set(features).issubset(previous_features):
                    fail(f"{key} package features for {consumer} may only shrink")
                for feature, common_features in features.items():
                    if not set(common_features).issubset(previous_features[feature]):
                        fail(f"{key} common features for {consumer}/{feature} may only shrink")


def main() -> None:
    baseline = read_json(BASELINE_PATH)
    if baseline.get("schema_version") != 1:
        fail("unsupported baseline schema")

    metadata = cargo_metadata()
    check_exact(
        "common feature list",
        actual_common_features(),
        sorted(baseline["common_features"]),
    )
    check_exact(
        "common public module list",
        actual_public_modules(),
        sorted(baseline["common_public_modules"]),
    )
    check_exact(
        "direct common dependencies",
        actual_dependencies(metadata),
        baseline["direct_dependencies"],
    )
    check_exact(
        "forwarded common features",
        actual_forwarded_common_features(metadata),
        baseline["forwarded_common_features"],
    )

    if len(sys.argv) == 3 and sys.argv[1] == "--base-ref":
        check_baseline_only_shrinks(baseline, sys.argv[2])
    elif len(sys.argv) != 1:
        fail("usage: check_baseline.py [--base-ref <git-ref>]")

    print("common decomposition baseline passed")


if __name__ == "__main__":
    main()
