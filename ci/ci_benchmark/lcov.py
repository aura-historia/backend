"""Validate and summarize the grcov LCOV records retained by the collector."""

import re
from collections import Counter


SUMMARIES = {"LF": "lines_found", "LH": "lines_hit", "FNF": "functions_found",
             "FNH": "functions_hit", "BRF": "branches_found", "BRH": "branches_hit"}


def unsigned(value, minimum=0):
    if not re.fullmatch(r"[0-9]+", value) or int(value) < minimum:
        raise ValueError(f"Invalid LCOV counter: {value}")
    return int(value)


def add_entry(record, prefix, value):
    if prefix == "DA":
        number, hits, *_ = value.split(",")
        entry = ((unsigned(number, 1),), unsigned(hits) > 0)
        record["lines"].append(entry)
    elif prefix == "FNDA":
        hits, name = value.split(",", 1)
        if not name:
            raise ValueError("Empty LCOV function name")
        record["functions"].append(((name,), unsigned(hits) > 0))
    elif prefix == "BRDA":
        number, block, branch, hits = value.split(",")
        entry = ((unsigned(number, 1), unsigned(block), unsigned(branch)), hits != "-" and unsigned(hits) > 0)
        record["branches"].append(entry)
    elif prefix == "FN":
        number, name = value.split(",", 1)
        unsigned(number, 1)
        if not name:
            raise ValueError("Empty LCOV function definition")
        record["definitions"].append(name)
    elif prefix in SUMMARIES:
        name = SUMMARIES[prefix]
        if name in record["summaries"]:
            raise ValueError(f"Repeated LCOV summary: {prefix}")
        record["summaries"][name] = unsigned(value)
    else:
        raise ValueError(f"Unsupported LCOV field: {prefix}")


def validate_record(record):
    # grcov emits all six summaries, including zero branch counters. Check entry
    # counts before unioning: distinct emitted functions can share a display name.
    observed = {name: value for kind in ("lines", "functions", "branches")
                for name, value in ((kind + "_found", len(record[kind])),
                                    (kind + "_hit", sum(hit for _, hit in record[kind])))}
    if record["summaries"] != observed:
        raise ValueError(f"Missing or inconsistent LCOV summaries: {record['source']}")
    if Counter(record["definitions"]) != Counter(identity[0] for identity, _ in record["functions"]):
        raise ValueError(f"LCOV function counts do not match definitions: {record['source']}")


def records(path):
    record = None
    for line in path.read_text().splitlines():
        if line.startswith("SF:"):
            if record is not None or not line[3:]:
                raise ValueError("Unterminated or empty LCOV source record")
            record = {"source": line[3:], "lines": [], "functions": [], "branches": [], "definitions": [], "summaries": {}}
        elif line == "end_of_record":
            if record is None:
                raise ValueError("LCOV record terminator has no source")
            validate_record(record)
            yield record
            record = None
        elif record is not None:
            prefix, separator, value = line.partition(":")
            if not separator:
                raise ValueError(f"Malformed LCOV field: {line}")
            add_entry(record, prefix, value)
        elif line and not line.startswith("TN:"):
            raise ValueError("Coverage output contains no LCOV source for a field")
    if record is not None:
        raise ValueError("Unterminated LCOV source record")


def coverage_totals(path):
    """Report validated record sums and file/line/function/branch identity unions."""
    files = set()
    unions = {kind: {} for kind in ("lines", "functions", "branches")}
    record_sums = {name: 0 for name in SUMMARIES.values()}
    for record in records(path):
        files.add(record["source"])
        for name, value in record["summaries"].items():
            record_sums[name] += value
        for kind, union in unions.items():
            for identity, hit in record[kind]:
                key = (record["source"], *identity)
                union[key] = union.get(key, False) or hit
    if not files:
        raise ValueError("Coverage output contains no LCOV source records")
    union = {"files": len(files), **{name: value for kind, entries in unions.items()
             for name, value in ((kind + "_found", len(entries)), (kind + "_hit", sum(entries.values())))}}
    return {"record_sums": record_sums, "identity_union": union,
            "union_note": "Function/branch identities follow emitted LCOV keys; differing compiler branch IDs across builds are not semantically reconciled"}
