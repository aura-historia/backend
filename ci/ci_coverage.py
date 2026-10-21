"""Normalize LLVM coverage into one source record per repository file."""

import pathlib

from ci_benchmark.lcov import unsigned


SUMMARY_FIELDS = {"LF", "LH", "FNF", "FNH", "BRF", "BRH"}


def source_path(value, root):
    path = pathlib.Path(value)
    if not path.is_absolute() and ".." in path.parts:
        raise ValueError(f"Coverage source escapes its root: {value}")
    path = (path if path.is_absolute() else root / path).resolve()
    try:
        relative = path.relative_to(root)
    except ValueError:
        return None
    if not relative.parts or relative.parts[0] != "src":
        return None
    if not path.is_file():
        raise ValueError(f"Coverage source is missing: {relative}")
    return relative.as_posix()


def empty_record():
    return {"lines": {}, "definitions": {}, "functions": {}, "branches": {}}


def add_count(entries, key, count):
    entries[key] = entries.get(key, 0) + count


def line_number(value, source, line_counts):
    number = unsigned(value, 1)
    if number > line_counts[source]:
        raise ValueError(f"Coverage line is outside source: {source}:{number}")
    return number


def read_counter(current, field, value, source, line_counts):
    if field == "DA":
        number, count, *_ = value.split(",")
        add_count(current["lines"], line_number(number, source, line_counts), unsigned(count))
    elif field == "FN":
        number, name = value.split(",", 1)
        number = line_number(number, source, line_counts)
        if not name or (name in current["definitions"] and current["definitions"][name] != number):
            raise ValueError(f"Conflicting LCOV function: {source}:{name}")
        current["definitions"][name] = number
    elif field == "FNDA":
        count, name = value.split(",", 1)
        if not name:
            raise ValueError("Empty LCOV function name")
        add_count(current["functions"], name, unsigned(count))
    elif field == "BRDA":
        number, block, branch, count = value.split(",")
        key = (line_number(number, source, line_counts), unsigned(block), unsigned(branch))
        add_count(current["branches"], key, 0 if count == "-" else unsigned(count))
    elif field in SUMMARY_FIELDS:
        unsigned(value)
    else:
        raise ValueError(f"Unsupported LCOV field: {field}")


def parse_lcov(text, root):
    """Read entry counters; LLVM's combined summary counters can disagree."""
    sources, line_counts, current, active = {}, {}, None, False
    for row in text.splitlines():
        if row.startswith("SF:"):
            if active or not row[3:]:
                raise ValueError("Unterminated or empty LCOV source record")
            active = True
            source = source_path(row[3:], root)
            current = sources.setdefault(source, empty_record()) if source else None
            if source and source not in line_counts:
                line_counts[source] = len((root / source).read_bytes().splitlines())
        elif row == "end_of_record":
            if not active:
                raise ValueError("LCOV record terminator has no source")
            current, active = None, False
        elif not active:
            if row and not row.startswith("TN:"):
                raise ValueError("LCOV field has no source")
        elif current is not None:
            field, separator, value = row.partition(":")
            if not separator:
                raise ValueError(f"Malformed LCOV field: {row}")
            read_counter(current, field, value, source, line_counts)
    if active:
        raise ValueError("Unterminated LCOV source record")
    if not sources or not any(record["lines"] for record in sources.values()):
        raise ValueError("Coverage output contains no repository line records")
    for source, record in sources.items():
        if record["definitions"].keys() != record["functions"].keys():
            raise ValueError(f"LCOV function definitions and counts differ: {source}")
    return sources


def validate_branch_origins(section, root):
    for function in section["functions"]:
        seen = set()
        for region in function["branches"]:
            source = source_path(function["filenames"][region[6]], root)
            if source is not None:
                location = (source, region[0], region[1])
                if location in seen:
                    raise ValueError(f"Ambiguous LLVM branch location within a function: {location}")
                seen.add(location)


def apply_branch_regions(sources, data, root):
    """Keep block zero; encode source column and outcome in a stable branch ID."""
    for record in sources.values():
        record["branches"] = {}
    spans = {}
    line_counts = {source: len((root / source).read_bytes().splitlines()) for source in sources}
    for section in data["data"]:
        validate_branch_origins(section, root)
        for file in section["files"]:
            source = source_path(file["filename"], root)
            if source is None:
                continue
            if source not in sources:
                raise ValueError(f"LLVM branch source has no LCOV record: {source}")
            for region in file["branches"]:
                if len(region) != 9:
                    raise ValueError("Unexpected LLVM branch region format")
                start, column, end, end_column, true, false, _, _, kind = region
                line_number(str(start), source, line_counts)
                key, span = (source, start, column), (end, end_column, kind)
                if key in spans and spans[key] != span:
                    raise ValueError(f"Ambiguous LLVM branch location: {key}")
                spans[key] = span
                column = unsigned(str(column), 1)
                if 2 * column + 1 > 2**31 - 1:
                    raise ValueError(f"LLVM branch column exceeds LCOV integer range: {column}")
                for branch, count in enumerate((true, false)):
                    add_count(sources[source]["branches"], (start, 0, 2 * column + branch), unsigned(str(count)))


def combine(sources, incoming):
    for source, record in incoming.items():
        target = sources.setdefault(source, empty_record())
        for name, number in record["definitions"].items():
            if name in target["definitions"] and target["definitions"][name] != number:
                raise ValueError(f"Conflicting LCOV function location: {source}:{name}")
            target["definitions"][name] = number
        for kind in ("lines", "functions", "branches"):
            for key, count in record[kind].items():
                add_count(target[kind], key, count)


def render(sources):
    rows = []
    for source, record in sorted(sources.items()):
        rows.append(f"SF:{source}")
        rows.extend(f"FN:{number},{name}" for name, number in sorted(record["definitions"].items()))
        rows.extend(f"FNDA:{count},{name}" for name, count in sorted(record["functions"].items()))
        rows.extend(f"DA:{number},{count}" for number, count in sorted(record["lines"].items()))
        rows.extend(f"BRDA:{number},{block},{branch},{count}" for (number, block, branch), count in sorted(record["branches"].items()))
        for kind, found, hit in (("functions", "FNF", "FNH"), ("lines", "LF", "LH"), ("branches", "BRF", "BRH")):
            rows.extend((f"{found}:{len(record[kind])}", f"{hit}:{sum(count > 0 for count in record[kind].values())}"))
        rows.append("end_of_record")
    return "\n".join(rows) + "\n"
