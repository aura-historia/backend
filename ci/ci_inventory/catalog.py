"""The live Cargo target set and the explicit, reviewed lane assignment contract."""

import json
import pathlib
import re
import tomllib

from .io import command
from .source import scan, source_paths


LANES = {
    "API": {"runner": "ubuntu-latest", "description": "API package targets"},
    "CRAWLER": {"runner": "ubuntu-latest", "description": "Crawler package targets"},
    "INFRASTRUCTURE": {"runner": "ubuntu-latest", "description": "Fixture, adapter, worker, Lambda and CDC targets"},
    "REMAINING": {"runner": "ubuntu-latest", "description": "All other targets"},
}


def lane_for(package, fixtures):
    if package == "aura-historia-api":
        return "API"
    if package == "crawler":
        return "CRAWLER"
    if fixtures or any(token in package for token in ("postgres", "opensearch", "lambda", "cdc", "worker")) or package == "test-api":
        return "INFRASTRUCTURE"
    return "REMAINING"


def target_record(package, target, root, member_roots):
    package_root = pathlib.Path(package["manifest_path"]).parent
    source_target = dict(target, src_path=pathlib.Path(target["src_path"]))
    signals = scan(source_paths(source_target, package_root, member_roots), root)
    kind = "+".join(target["kind"])
    identity = f"{package['name']}::{kind}::{target['name']}"
    manifest = tomllib.loads(pathlib.Path(package["manifest_path"]).read_text())
    entries = [manifest.get("lib", {})] if any(kind in {"lib", "proc-macro"} for kind in target["kind"]) else manifest.get(kind, [])
    settings = next((entry for entry in entries if entry.get("name", target["name"]) == target["name"]), {})
    return {"id": identity, "name": target["name"], "kind": target["kind"],
            "source": str(source_target["src_path"].relative_to(root)),
            "test": target["test"], "doctest": target["doctest"],
            "required_features": target.get("required-features", []),
            "harness": settings.get("harness", True),
            "lane": lane_for(package["name"], signals["fixtures"]),
            "runner": "ubuntu-latest", "source_signals": signals,
            "discovery": {"status": "NOT_DISCOVERED", "test_count": None, "ignored_count": None},
            "doctest_discovery": {"status": "NOT_DISCOVERED" if target["doctest"] else "NOT_APPLICABLE", "test_count": None}}


def build_catalog(metadata, root):
    members = set(metadata["workspace_members"])
    packages = sorted((p for p in metadata["packages"] if p["id"] in members), key=lambda p: p["name"])
    member_roots = {pathlib.Path(p["manifest_path"]).parent for p in packages}
    records, assignments = [], {}
    for package in packages:
        path = str(pathlib.Path(package["manifest_path"]).parent.relative_to(root))
        targets = [target_record(package, target, root, member_roots) for target in package["targets"]]
        records.append({"name": package["name"], "version": package["version"], "path": path,
                        "features": package["features"], "targets": targets,
                        "current_ci_eligible": path not in {".", "src/test-api/src/test-api-macros"}})
        assignments[package["name"]] = {"path": path, "targets": [
            {key: target[key] for key in ("id", "lane", "test", "doctest", "required_features", "harness")} for target in targets]}
    manifest = {"schema_version": 1, "purpose": "INVENTORY_ONLY", "lanes": LANES, "packages": assignments}
    summary = {"packages": len(records), "targets": sum(len(p["targets"]) for p in records),
               "doctest_targets": sum(t["doctest"] for p in records for t in p["targets"]),
               "test_count": None, "ignored_count": None, "doctest_count": None}
    return {"schema_version": 1, "summary": summary, "packages": records,
            "feature_comparison": {"status": "NOT_COLLECTED"},
            "discovery": {"status": "NOT_DISCOVERED"}}, manifest


def check_manifest(manifest, expected):
    if manifest != expected:
        differences = []
        for name in sorted(set(manifest.get("packages", {})) | set(expected["packages"])):
            if manifest.get("packages", {}).get(name) != expected["packages"].get(name):
                differences.append(name)
        raise ValueError("Package/target/lane contract changed: " + ", ".join(differences or ["schema/lanes"]) +
                         ". Review the delta, then explicitly run --write-manifest.")


def package_owners(manifest, expected):
    """Keep reviewed target classifications while assigning whole packages."""
    check_manifest(manifest, expected)
    owners, seen = {}, set()
    for name, package in sorted(manifest["packages"].items()):
        if not package["targets"]:
            raise ValueError(f"Package has no target ownership: {name}")
        classifications = set()
        for target in package["targets"]:
            identity = target["id"]
            parts = identity.split("::")
            if len(parts) != 3 or not all(parts) or parts[0] != name:
                raise ValueError(f"Target does not belong to package {name}: {identity}")
            if identity in seen:
                raise ValueError(f"Duplicate target ownership: {identity}")
            seen.add(identity)
            classifications.add(target["lane"])
        exclusive = {"aura-historia-api": "API", "crawler": "CRAWLER"}.get(name)
        if exclusive:
            if classifications != {exclusive}:
                raise ValueError(f"Invalid exclusive package classification: {name}")
            owners[name] = exclusive
        elif classifications <= {"INFRASTRUCTURE", "REMAINING"}:
            owners[name] = "INFRASTRUCTURE" if "INFRASTRUCTURE" in classifications else "REMAINING"
        else:
            raise ValueError(f"Invalid package classification: {name}")
    return owners


def lane_selection(manifest, expected, lane):
    """Emit package arguments and complete target ownership for one lane."""
    if lane not in LANES:
        raise ValueError(f"Unknown test lane: {lane}")
    owners = package_owners(manifest, expected)
    packages = sorted(name for name, owner in owners.items() if owner == lane)
    if not packages:
        raise ValueError(f"Test lane has no packages: {lane}")
    targets = [target for name in packages for target in manifest["packages"][name]["targets"]]
    return {"lane": lane, "packages": packages,
            "package_args": [argument for name in packages for argument in ("--package", name)],
            "targets": sorted(target["id"] for target in targets),
            "doctest_targets": sorted(target["id"] for target in targets if target["doctest"])}


def tree_features(argv, cwd, output):
    raw = command(["cargo", "tree", "--locked", "--all-features", "--prefix", "none", "--format", "{p}|{f}",
                   "--edges", "normal,build,dev", *argv], cwd)
    output.write_text(raw)
    features = {}
    for line in raw.splitlines():
        match = re.fullmatch(r"([^ ]+ v[^ ]+)(?: .*?)?\|([^ ]*)(?: \(\*\))?", line)
        if match:
            features.setdefault(match[1], set()).update(filter(None, match[2].split(",")))
    if not features:
        raise ValueError("Cargo tree returned no feature records")
    return features


def feature_inventory(packages, root, output):
    directory = output / "features"
    directory.mkdir(exist_ok=True)
    workspace = tree_features(["--workspace"], root, directory / "workspace.txt")
    comparisons = {}
    for package in packages:
        local = tree_features([], root / package["path"], directory / (package["name"] + ".txt"))
        comparisons[package["name"]] = {"scope": "ALL_FEATURES_FROM_PACKAGE_DIRECTORY",
            "current_ci_eligible": package["current_ci_eligible"],
            "package_features": sorted(local.get(package["name"] + " v" + package["version"], set())),
            "differences": [{"dependency": name, "package_features": sorted(features),
                             "workspace_features": sorted(workspace.get(name, set())),
                             "workspace_only": sorted(workspace.get(name, set()) - features)}
                            for name, features in sorted(local.items()) if features != workspace.get(name, set())]}
    return {"status": "COLLECTED", "method": "cargo tree --locked --all-features --edges normal,build,dev",
            "limitation": "Resolver graph view; host/build feature contexts are unioned by package/version. Compiled discovery owns executable cases.",
            "packages": comparisons}
