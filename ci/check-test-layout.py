#!/usr/bin/env python3
"""Fail when crate sources keep tests outside the file that contains the code they test.

Unit tests belong in a trailing `#[cfg(test)] mod tests { ... }` of the tested file; black-box
tests use the crate's `tests/` directory (docs/arch.md, test visibility and testing strategy).
"""

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
# Reviewed exceptions. Add an entry only with a reason a reviewer can check.
ALLOWED = {
    # Scenario suite of the scraper orchestrator: one scenario per file over crate-private mocks.
    "src/crawler/src/scraper/scraper_service/tests",
    # Manual, opt-in provider benchmark harness rather than unit tests.
    "src/search-filter-service/src/product_match_evaluator/batching/benchmark.rs",
}
# Test-only helpers shared by several files of one crate; such modules contain no tests.
SUPPORT_MODULES = {"test_support"}
TEST_FILE = re.compile(r"^(?:tests|.+_tests?)\.rs$")
DECLARATION = re.compile(
    r"(?P<attributes>(?:^[ \t]*#\[[^\n]*\][ \t]*\n)+)"
    r"^[ \t]*(?:pub(?:\([^)]*\))?[ \t]+)?mod[ \t]+(?P<name>\w+)[ \t]*;",
    re.M,
)
TEST_ONLY = re.compile(r"#\[cfg\(\s*(?:test\s*\)|all\(\s*test\s*,)")
PATH = re.compile(r'#\[path\s*=\s*"([^"]+)"\]')


def relative(path):
    return path.relative_to(ROOT).as_posix()


def allowed(path):
    name = relative(path)
    return any(name == entry or name.startswith(entry + "/") for entry in ALLOWED)


def module_path(declaring_file, name, attributes):
    explicit = PATH.search(attributes)
    if explicit:
        return declaring_file.parent / explicit.group(1)
    base = declaring_file.parent
    if declaring_file.name not in ("mod.rs", "lib.rs", "main.rs"):
        base = base / declaring_file.stem
    candidate = base / f"{name}.rs"
    return candidate if candidate.exists() else base / name / "mod.rs"


def violations():
    for source in sorted(ROOT.glob("src/*/src")):
        for path in sorted(source.rglob("*")):
            if allowed(path):
                continue
            if path.is_dir() and path.name == "tests":
                yield f"{relative(path)}: test module directory inside crate sources"
            elif path.is_file() and TEST_FILE.match(path.name):
                yield f"{relative(path)}: separate test file inside crate sources"
            elif path.is_file() and path.suffix == ".rs":
                for match in DECLARATION.finditer(path.read_text()):
                    name, attributes = match.group("name"), match.group("attributes")
                    if not TEST_ONLY.search(attributes) or name in SUPPORT_MODULES:
                        continue
                    target = module_path(path, name, attributes)
                    if not allowed(target):
                        line = path.read_text().count("\n", 0, match.start("name")) + 1
                        yield f"{relative(path)}:{line}: test-only module `{name}` lives in {relative(target)}"


def main():
    found = list(violations())
    for violation in found:
        print(violation, file=sys.stderr)
    if found:
        print(
            "Move unit tests into a trailing `#[cfg(test)] mod tests { ... }` of the tested file, "
            "or public-API tests into the crate's tests/ directory (docs/arch.md).",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
