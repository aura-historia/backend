"""Parse measured values; unavailable observations remain explicit nulls."""

import json
import re


def observations(path):
    raw = path.read_text()
    summaries = [dict(zip(("passed", "failed", "ignored", "measured", "filtered"), map(int, match))) for match in re.findall(
        r"test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out", raw)]
    totals = {key: sum(summary[key] for summary in summaries) for key in ("passed", "failed", "ignored", "measured", "filtered")}
    phases = {"SETUP": [], "BODY": [], "TEARDOWN": [], "FLOCI_STARTUP": [], "POSTGRES_STARTUP": [],
              "POSTGRES_MIGRATIONS": [], "POSTGRES_RESET": [], "OPENSEARCH_STARTUP": [], "OPENSEARCH_RESET": []}
    records = []
    for line in raw.splitlines():
        match = re.search(r"AURA_CI_TIMING (\{.*\})", line)
        if match:
            record = json.loads(match[1])
            if record["phase"] not in phases or record["elapsed_seconds"] < 0:
                raise ValueError("Invalid benchmark timing observation")
            phases[record["phase"]].append(record["elapsed_seconds"])
            records.append(record)
    return {"tests": {"status": "COLLECTED" if summaries else "UNAVAILABLE", "totals": totals if summaries else None,
                      "harness_summaries": summaries},
            "fixture_timings": {"status": "COLLECTED" if records else "NOT_OBSERVED", "seconds": phases, "records": records,
                "scope": "Opt-in fixture readiness/startup/reset and aura_integration_test setup/body/teardown. Setup includes Postgres/OpenSearch startup/migrations; teardown includes resets. Nested phases overlap and must not be added together. Body excludes fixture setup/teardown but includes helpers called by the test.",
                "limitation": "Ordinary libtest tests have no per-body boundary; failed/cancelled phases may not emit a completion. Empty samples are not zero-duration phases."}}


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
