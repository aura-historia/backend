"""Capture the Cargo objects produced by each compiling lane phase."""

import hashlib
import json
import pathlib
import shutil
import sys

from .process import execute


def record_artifacts(raw, destination, objects):
    destination.unlink(missing_ok=True)
    messages = []
    for line in raw.splitlines():
        item = json.loads(line)
        if not isinstance(item, dict) or "reason" not in item:
            raise ValueError("Expected Cargo metadata before build-finished")
        messages.append(item)
        if item["reason"] == "build-finished":
            break
    if (not messages or messages[-1].get("reason") != "build-finished"
            or messages[-1].get("success") is not True
            or not any(item["reason"] == "compiler-artifact" for item in messages)):
        raise ValueError(f"Cargo coverage artifact metadata is incomplete: {destination}")
    for item in messages:
        # Cargo's non-test executable paths are mutable aliases. A later package
        # selection can replace one while a supervisor still runs the old object.
        if item["reason"] == "compiler-artifact" and item.get("executable") and item.get("profile", {}).get("test") is False:
            original = pathlib.Path(item["executable"]).resolve()
            with original.open("rb") as stream:
                digest = hashlib.file_digest(stream, "sha256").hexdigest()
            retained = objects / digest
            retained.parent.mkdir(parents=True, exist_ok=True)
            if not retained.exists():
                shutil.copy2(original, retained)
            item.update(original_executable=str(original), executable=str(retained.resolve()), coverage_sha256=digest)
    destination.write_text("".join(json.dumps(item) + "\n" for item in messages))


def execute_cargo(argv, root, environment, destination, *, junit=None):
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.unlink(missing_ok=True)
    raw = destination.with_suffix(".stdout.log")
    failed = False
    try:
        with raw.open("wb") as stream:
            execute(argv, root, environment, junit=junit, stdout=stream)
    except BaseException:
        failed = True
        raise
    finally:
        try:
            if raw.is_file():
                with raw.open(encoding="utf-8", errors="replace") as stream:
                    shutil.copyfileobj(stream, sys.stdout)
                sys.stdout.flush()
        except (OSError, UnicodeError) as error:
            if not failed:
                raise
            print(f"test lane: could not replay Cargo stdout: {error}", file=sys.stderr)
    record_artifacts(raw.read_bytes(), destination, pathlib.Path(environment["AURA_CI_COVERAGE_DIR"]) / "objects")
