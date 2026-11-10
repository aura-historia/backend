#!/usr/bin/env python3
"""Analyze a local normalization corpus and its replays. Reads only local files.

    analyze.py summary REPLAY                       # statuses and recorded issue table
    analyze.py tally   --work DIR REPLAY FIELD REASON [--limit N]
                                                    # distinct raw values behind a CURRENT issue
    analyze.py labels  --work DIR HOST [HOST ...]   # every availability label a dealer uses
    analyze.py diff    BEFORE AFTER                 # regressions; exit 1 when something needs review
    analyze.py compare BEFORE AFTER                 # Markdown table for the pull request
    analyze.py fixtures SPEC.json --replay-bin BIN [--append FIXTURES.jsonl]
                                                    # minimized synthetic reviewed fixtures

DIR holds corpus.jsonl and sources.tsv from prod_corpus.py. REPLAY files are the JSONL output of
`normalization-replay --input DIR/corpus.jsonl --output FILE`.
"""

import argparse
from collections import Counter, defaultdict
import json
from pathlib import Path
import subprocess
import sys
import tempfile

RAW_KEY = {"AVAILABILITY": "availability", "PRICE": "price", "TITLE": "title",
           "DESCRIPTION": "description", "SHIPPING": "description"}
STATUS_RANK = {"RESOLVED": 0, "UNRESOLVED": 1, "INVALID": 2, "UNAVAILABLE_EVIDENCE": 3,
               "INCOMPATIBLE": 3, "SYSTEM_ERROR": 3}
REVIEWED_FIELDS = ("availability", "price", "priceEstimateMin", "priceEstimateMax", "shipping",
                   "title", "description", "url", "images")


def read_jsonl(path):
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            if line.strip():
                yield json.loads(line)


def replay_cases(path):
    return {record["caseId"]: record for record in read_jsonl(path) if record.get("type") == "case"}


def replay_summary(path):
    return next(record for record in read_jsonl(path) if record.get("type") == "summary")


def issue_keys(record):
    return {(issue["field"], issue["reason"]) for issue in record.get("issues") or []}


def hosts(work):
    path = Path(work) / "sources.tsv"
    if not path.exists():
        return {}
    mapping = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        parts = line.split("\t")
        if len(parts) == 4 and parts[0] != "listing_source_id":
            mapping[parts[0]] = parts[2]
    return mapping


def is_decided(value):
    """Whether a replayed field holds an applied decision rather than an abstention."""
    if not isinstance(value, dict):
        return False
    if "kind" in value:
        return value["kind"] == "RESOLVED"
    return value.get("action") == "SET" and value.get("value") not in ("UNSUPPORTED", "NO_ASSERTION")


def regressions(before, after):
    """Case-by-case differences that need a human explanation before a change is accepted."""
    report = {"missing": [], "worse": Counter(), "new_issues": Counter(), "changed_decisions": Counter(),
              "reattributed": Counter(), "examples": defaultdict(list)}
    for case_id, old in before.items():
        new = after.get(case_id)
        if new is None:
            report["missing"].append(case_id)
            continue
        if STATUS_RANK[new["status"]] > STATUS_RANK[old["status"]]:
            report["worse"][(old["status"], new["status"])] += 1
            report["examples"]["worse"].append(case_id)
        old_issues, new_issues = issue_keys(old), issue_keys(new)
        for key in new_issues - old_issues:
            old_reasons = sorted(reason for field, reason in old_issues if field == key[0])
            if old_reasons:
                report["reattributed"][(key[0], ",".join(old_reasons), key[1])] += 1
            else:
                report["new_issues"][key] += 1
                report["examples"]["new:" + "/".join(key)].append(case_id)
        old_fields, new_fields = old.get("fields") or {}, new.get("fields") or {}
        for field in REVIEWED_FIELDS:
            if is_decided(old_fields.get(field)) and old_fields.get(field) != new_fields.get(field):
                report["changed_decisions"][field] += 1
                report["examples"]["changed:" + field].append(case_id)
    return report


def needs_review(report):
    return bool(report["missing"] or report["worse"] or report["new_issues"] or report["changed_decisions"])


def comparison_rows(before_summary, after_summary):
    reasons = sorted(set(before_summary["recordedIssues"]) | set(after_summary["recordedIssues"]))
    rows = []
    for reason in reasons:
        old = before_summary["recordedIssues"].get(reason, {})
        new = after_summary["recordedIssues"].get(reason, {})
        rows.append((reason, sum(new.values()) or sum(old.values()), old.get("RESOLVED", 0), new.get("RESOLVED", 0)))
    return rows


def raw_value(case, field):
    values = case["evidence"].get("input", {}).get("rawValues", {})
    value = values.get(RAW_KEY[field], {}).get("value")
    if field == "PRICE":
        value = [value, values.get("priceFormat")]
    return json.dumps(value, ensure_ascii=False)


def cmd_summary(args):
    summary = replay_summary(args.replay)
    print(f"cases {summary['cases']} completed {summary['completed']} evaluator v{summary['rawValuesEvaluatorVersion']}")
    print("statuses", json.dumps(summary["statuses"], sort_keys=True))
    for reason, statuses in sorted(summary["recordedIssues"].items()):
        print(f"  {reason:52} {json.dumps(statuses, sort_keys=True)}")
    return 0


def cmd_tally(args):
    wanted = {case_id for case_id, record in replay_cases(args.replay).items()
              if (args.field, args.reason) in issue_keys(record)}
    names = hosts(args.work)
    values, by_source = Counter(), defaultdict(Counter)
    for case in read_jsonl(Path(args.work) / "corpus.jsonl"):
        if case["caseId"] in wanted:
            value = raw_value(case, args.field)
            values[value] += 1
            by_source[value][names.get(case["listingSourceId"], case["listingSourceId"][:8])] += 1
    print(f"{len(wanted)} cases, {len(values)} distinct values")
    for value, count in values.most_common(args.limit):
        sources = ", ".join(f"{host}:{n}" for host, n in by_source[value].most_common(3))
        print(f"{count:6d}  {value[:150]}   <- {sources}")
    return 0


def cmd_labels(args):
    names = hosts(args.work)
    labels = defaultdict(Counter)
    for case in read_jsonl(Path(args.work) / "corpus.jsonl"):
        host = names.get(case["listingSourceId"])
        if host in args.hosts:
            availability = case["evidence"].get("input", {}).get("rawValues", {}).get("availability", {})
            label = availability.get("value") if availability.get("action") == "SET" else f"<{availability.get('action')}>"
            labels[host][label] += 1
    for host in args.hosts:
        print(host, "=>", "; ".join(f"{json.dumps(label, ensure_ascii=False)[:70]}:{n}"
                                    for label, n in labels[host].most_common(10)))
    return 0


def cmd_diff(args):
    report = regressions(replay_cases(args.before), replay_cases(args.after))
    print("missing cases:", len(report["missing"]))
    print("worse status:", dict(report["worse"]) or "none")
    print("new issues on cases that had none for the field:", dict(report["new_issues"]) or "none")
    print("changed decided fields:", dict(report["changed_decisions"]) or "none")
    print("re-attributed reasons (field, before, now):")
    for key, count in report["reattributed"].most_common():
        print(f"  {count:6d} {key}")
    examples = Path(args.after).with_suffix(".review.json")
    examples.write_text(json.dumps(report["examples"]))
    examples.chmod(0o600)
    print(f"example case ids: {examples}")
    if needs_review(report):
        print("REVIEW REQUIRED: explain or fix every item above before accepting the change")
        return 1
    return 0


def cmd_compare(args):
    before, after = replay_summary(args.before), replay_summary(args.after)
    print("| Recorded issue | Cases | Resolved before | Resolved now |")
    print("|---|---:|---:|---:|")
    for reason, cases, old, new in comparison_rows(before, after):
        print(f"| `{reason.split('/', 1)[1]}` | {cases:,} | {old:,} | {new:,} |")
    statuses = sorted(set(before["statuses"]) | set(after["statuses"]), key=lambda s: STATUS_RANK.get(s, 9))
    print()
    print("Case statuses: " + ", ".join(
        f"`{s}` {before['statuses'].get(s, 0):,} → {after['statuses'].get(s, 0):,}" for s in statuses) + ".")
    return 0


def synthetic_case(spec):
    """A minimized reviewed-fixture case: invented text, the observed raw value, no prod identifiers."""
    def patch(value):
        return {"action": "SET", "value": value} if value is not None else {"action": "UNCHANGED"}
    raw_values = {
        "sourceListingId": "item-1",
        "title": {"action": "SET", "value": spec["title"]},
        "description": {"action": "SET", "value": [spec["description"]]},
        "priceFormat": spec.get("priceFormat", "DISPLAY_TEXT"),
        "price": patch(spec.get("price")),
        "priceEstimateMin": patch(spec.get("priceEstimateMin")),
        "priceEstimateMax": patch(spec.get("priceEstimateMax")),
        "availability": patch(spec.get("availability")),
        "url": {"action": "SET", "value": "item-1"},
        "images": {"action": "SET", "value": []},
    }
    if spec.get("attributes"):
        raw_values["attributes"] = spec["attributes"]
    context = {"baseUrl": "https://example.test/"}
    if spec.get("fallbackCurrency"):
        context["fallbackCurrency"] = spec["fallbackCurrency"]
    return {
        "caseId": spec["name"], "entryPoint": "RAW_VALUES_NORMALIZATION_V1", "inputSha256": "",
        "listingSourceId": "", "recordedOutcome": "APPLIED", "recordedIssues": [],
        "evidence": {"kind": "RAW_REVISION", "productListingRawRevisionId": "", "input": {
            "operation": "UPSERT", "payloadFormat": "CRAWLER_EXTRACTED_PRODUCT", "payloadSchemaVersion": 1,
            "rawValuesSchemaVersion": 1, "sourcePayload": {}, "rawValues": raw_values,
            "normalizationContext": context}},
    }


def fixture_line(spec, result):
    expected = {
        "status": result["status"],
        "decision": result["decision"],
        "issues": [f"{issue['field']}/{issue['reason']}" for issue in result["issues"]],
        "fields": {field: result["fields"][field] for field in spec["fields"]},
    }
    # The existing fixture file uses json.dumps defaults (", " and ": " separators).
    return json.dumps({"name": spec["name"], "category": spec["category"], "case": synthetic_case(spec),
                       "expected": expected}, ensure_ascii=False)


def cmd_fixtures(args):
    specs = json.loads(Path(args.spec).read_text(encoding="utf-8"))
    for spec in specs:
        if spec["category"] not in ("REPAIRED", "SUCCESSFUL", "NEGATIVE"):
            raise SystemExit(f"{spec['name']}: unknown category {spec['category']}")
    with tempfile.TemporaryDirectory() as directory:
        cases, output = Path(directory) / "cases.jsonl", Path(directory) / "replay.jsonl"
        cases.write_text("".join(json.dumps(synthetic_case(spec), ensure_ascii=False) + "\n" for spec in specs))
        subprocess.run([args.replay_bin, "--input", str(cases), "--output", str(output)],
                       check=True, stdout=subprocess.DEVNULL)
        results = replay_cases(output)
    lines = []
    for spec in specs:
        result = results[spec["name"]]
        fields = json.dumps({f: result["fields"].get(f) for f in spec["fields"]}, ensure_ascii=False)
        issues = [f"{issue['field']}/{issue['reason']}" for issue in result["issues"]]
        print(f"{spec['category']:10} {spec['name']}\n    {result['status']} {issues} {fields}")
        lines.append(fixture_line(spec, result))
    if args.append:
        target = Path(args.append)
        existing = target.read_text(encoding="utf-8")
        names = {json.loads(line)["name"] for line in existing.splitlines() if line.strip()}
        duplicates = names & {spec["name"] for spec in specs}
        if duplicates:
            raise SystemExit(f"fixture names already exist: {sorted(duplicates)}")
        with target.open("a", encoding="utf-8") as handle:
            if existing and not existing.endswith("\n"):
                handle.write("\n")
            handle.write("\n".join(lines) + "\n")
        print(f"appended {len(lines)} fixtures to {target}")
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    summary = commands.add_parser("summary")
    summary.add_argument("replay")
    tally = commands.add_parser("tally")
    tally.add_argument("--work", required=True)
    tally.add_argument("replay")
    tally.add_argument("field", choices=sorted(RAW_KEY))
    tally.add_argument("reason")
    tally.add_argument("--limit", type=int, default=60)
    labels = commands.add_parser("labels")
    labels.add_argument("--work", required=True)
    labels.add_argument("hosts", nargs="+")
    for name in ("diff", "compare"):
        sub = commands.add_parser(name)
        sub.add_argument("before")
        sub.add_argument("after")
    fixtures = commands.add_parser("fixtures")
    fixtures.add_argument("spec")
    fixtures.add_argument("--replay-bin", required=True)
    fixtures.add_argument("--append")
    args = parser.parse_args(argv)
    handler = {"summary": cmd_summary, "tally": cmd_tally, "labels": cmd_labels, "diff": cmd_diff,
               "compare": cmd_compare, "fixtures": cmd_fixtures}[args.command]
    return handler(args)


if __name__ == "__main__":
    sys.exit(main())
