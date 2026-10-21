"""Parse measured values; unavailable observations remain explicit nulls."""

import json
import re

from .lcov import coverage_totals


def observations(path):
    raw = path.read_text()
    summaries = [dict(zip(("passed", "failed", "ignored", "measured", "filtered"), map(int, match))) for match in re.findall(
        r"test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out", raw)]
    totals = {key: sum(summary[key] for summary in summaries) for key in ("passed", "failed", "ignored", "measured", "filtered")}
    phases = {"SETUP": [], "BODY": [], "TEARDOWN": [], "FLOCI_STARTUP": [], "POSTGRES_STARTUP": [],
              "POSTGRES_MIGRATIONS": [], "POSTGRES_RESET": [], "OPENSEARCH_STARTUP": [], "OPENSEARCH_RESET": [],
              "CONTEXT_ACQUIRE": [], "POSTGRES_SLOT_RESET": [], "CONTEXT_CLEANUP": []}
    records = []
    for number, line in enumerate(raw.splitlines(), 1):
        _, marker, payload = line.partition("AURA_CI_TIMING ")
        if marker:
            try:
                record = json.loads(payload)
                if record["phase"] not in phases or record["elapsed_seconds"] < 0:
                    raise ValueError("Invalid benchmark timing observation")
            except (ValueError, KeyError, TypeError) as error:
                raise ValueError(f"Malformed benchmark timing record on line {number}") from error
            phases[record["phase"]].append(record["elapsed_seconds"])
            records.append(record)
    return {"tests": {"status": "COLLECTED" if summaries else "UNAVAILABLE", "totals": totals if summaries else None,
                      "harness_summaries": summaries},
            "fixture_timings": {"status": "COLLECTED" if records else "NOT_OBSERVED", "seconds": phases, "records": records,
                "scope": "Opt-in fixture readiness/startup/reset and aura_integration_test setup/body/teardown. Setup includes Postgres/OpenSearch startup/migrations; teardown includes legacy resets. Context acquisition includes lease waits/recovery, slot reset includes baseline preparation/cloning, and context cleanup includes owned shutdown/resource removal. Nested phases overlap and must not be added together. Body excludes fixture setup/teardown but includes helpers called by the test.",
                "limitation": "Ordinary libtest tests have no per-body boundary; failed/cancelled phases may not emit a completion. Empty samples are not zero-duration phases."}}
