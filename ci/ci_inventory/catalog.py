"""Every workspace package belongs to exactly one reviewed CI lane."""

import json

from .io import command


LANES = ("API", "CRAWLER", "INFRASTRUCTURE", "REMAINING")


def workspace_packages(root):
    metadata = json.loads(command(["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"], root))
    members = set(metadata["workspace_members"])
    return {package["name"]: package for package in metadata["packages"] if package["id"] in members}


def check_manifest(manifest, packages):
    if not isinstance(manifest, dict) or set(manifest) != set(LANES):
        raise ValueError("The lane manifest must contain exactly the four CI lanes")
    owned = set()
    for lane, names in manifest.items():
        if not isinstance(names, list) or not names or any(not isinstance(name, str) for name in names):
            raise ValueError(f"Lane {lane} must contain a nonempty package list")
        for name in names:
            if name in owned:
                raise ValueError(f"Duplicate package ownership: {name}")
            owned.add(name)
    missing, unknown = set(packages) - owned, owned - set(packages)
    if missing or unknown:
        raise ValueError(f"Review ci/test-lanes.json: unassigned packages {sorted(missing)}; unknown packages {sorted(unknown)}")


def target_identity(package, target):
    return f"{package}::{'+'.join(target['kind'])}::{target['name']}"


def lane_selection(manifest, packages, lanes):
    check_manifest(manifest, packages)
    if not lanes or len(lanes) != len(set(lanes)) or not set(lanes) <= set(LANES):
        raise ValueError("Select distinct, nonempty test lanes")
    names = sorted(name for lane in lanes for name in manifest[lane])
    targets = [(name, target) for name in names for target in packages[name]["targets"]]
    return {"lanes": sorted(lanes), "packages": names,
            "package_args": [argument for name in names for argument in ("--package", name)],
            "targets": sorted(target_identity(name, target) for name, target in targets),
            "doctest_targets": sorted(target_identity(name, target) for name, target in targets if target["doctest"])}
