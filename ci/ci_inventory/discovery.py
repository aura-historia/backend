"""Check compiled Nextest suites against their Cargo owners and checkout."""

import os
import pathlib

from .catalog import target_identity


def validate_suites(data, packages, selection):
    expected = {target_identity(name, target) for name in selection["packages"]
                for target in packages[name]["targets"] if target["test"]}
    suites = {}
    for suite in data["rust-suites"].values():
        identity = f"{suite['package-name']}::{suite['kind']}::{suite['binary-name']}"
        if identity not in expected or identity in suites:
            raise ValueError(f"Duplicate or foreign lane suite: {identity}")
        package_root = pathlib.Path(packages[suite["package-name"]]["manifest_path"]).parent
        if pathlib.Path(suite["cwd"]).resolve() != package_root.resolve():
            raise ValueError(f"Nextest suite belongs to another checkout: {identity}")
        suites[identity] = suite
    if set(suites) != expected:
        raise ValueError(f"Cargo test targets disappeared from Nextest: {sorted(expected - suites.keys())}")
    return suites


def validate_discovered_supervisor(data, supervisor):
    package_ids = {suite["package-id"] for suite in data["rust-suites"].values()
                   if suite["package-name"] == "test-api"}
    build = data["rust-build-meta"]
    binaries = [binary for package in package_ids for binary in build["non-test-binaries"].get(package, [])
                if binary["name"] == "aura-test-env"]
    if len(package_ids) != 1 or len(binaries) != 1:
        raise ValueError("Discovery must produce one runnable test-api supervisor")
    binary = binaries[0]
    path = (pathlib.Path(build["target-directory"]) / binary["path"]).resolve()
    if (binary["kind"] != "bin-exe" or binary["build-platform"] != "target"
            or path != supervisor.resolve() or not path.is_file() or not os.access(path, os.X_OK)):
        raise ValueError(f"Discovered test-api supervisor is not the requested executable: {supervisor}")
