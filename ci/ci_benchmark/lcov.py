"""Validate and summarize emitted LCOV coverage records."""

def coverage_totals(path):
    """Report both LCOV record sums and identity-unioned totals.

    Concatenation can repeat source files. The union key is file+line,
    file+function name or file+line/block/branch; hits use logical OR.
    """
    files, lines, functions, branches = set(), {}, {}, {}
    record_sums = {"lines_found": 0, "lines_hit": 0, "functions_found": 0, "functions_hit": 0, "branches_found": 0, "branches_hit": 0}
    source = None
    for line in path.read_text().splitlines():
        if line.startswith("SF:"):
            source = line[3:]
            files.add(source)
        elif line == "end_of_record":
            source = None
        elif source is not None:
            if line.startswith("DA:"):
                number, hits, *_ = line[3:].split(",")
                key = (source, number)
                lines[key] = lines.get(key, False) or int(hits) > 0
            elif line.startswith("FNDA:"):
                hits, name = line[5:].split(",", 1)
                key = (source, name)
                functions[key] = functions.get(key, False) or int(hits) > 0
            elif line.startswith("BRDA:"):
                number, block, branch, hits = line[5:].split(",")
                key = (source, number, block, branch)
                branches[key] = branches.get(key, False) or (hits != "-" and int(hits) > 0)
            else:
                for prefix, name in (("LF:", "lines_found"), ("LH:", "lines_hit"), ("FNF:", "functions_found"), ("FNH:", "functions_hit"), ("BRF:", "branches_found"), ("BRH:", "branches_hit")):
                    if line.startswith(prefix):
                        record_sums[name] += int(line[len(prefix):])
    if not files:
        raise ValueError("Coverage output contains no LCOV source records")
    union = {"files": len(files), "lines_found": len(lines), "lines_hit": sum(lines.values()),
             "functions_found": len(functions), "functions_hit": sum(functions.values()),
             "branches_found": len(branches), "branches_hit": sum(branches.values())}
    return {"record_sums": record_sums, "identity_union": union,
            "union_note": "Function/branch identities follow emitted LCOV keys; differing compiler branch IDs across builds are not semantically reconciled"}
