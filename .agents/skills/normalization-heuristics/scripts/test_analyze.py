"""Regression classification, PR table and fixture generation, on synthetic replay records."""

import json
from pathlib import Path
import unittest

from analyze import capture_time, comparison_rows, fixture_line, is_decided, needs_review, regressions, synthetic_case

FIXTURES = Path(__file__).resolve().parents[4] / (
    "src/product-listing-normalization-replay/tests/fixtures/reviewed_cases.jsonl")


def record(case_id, status, issues=(), **fields):
    return {"type": "case", "caseId": case_id, "status": status,
            "issues": [{"field": f, "reason": r} for f, r in issues], "fields": fields}


AVAILABLE = {"action": "SET", "value": "AVAILABLE"}
UNSUPPORTED = {"action": "SET", "value": "UNSUPPORTED"}


class DecidedTest(unittest.TestCase):
    def test_should_count_only_applied_values_as_decisions(self):
        self.assertTrue(is_decided(AVAILABLE))
        self.assertTrue(is_decided({"kind": "RESOLVED", "worldwide": True}))
        for value in (UNSUPPORTED, {"action": "SET", "value": "NO_ASSERTION"}, {"action": "UNCHANGED"},
                      {"kind": "NO_ASSERTION"}, {"kind": "UNSUPPORTED"}, None, {"error": "x"}):
            self.assertFalse(is_decided(value), value)


class RegressionTest(unittest.TestCase):
    def test_should_accept_repairs_and_reattributed_abstentions(self):
        before = {
            "a": record("a", "UNRESOLVED", [("AVAILABILITY", "AVAILABILITY_UNRECOGNIZED")], availability=UNSUPPORTED),
            "b": record("b", "UNRESOLVED", [("SHIPPING", "SHIPPING_CONDITIONAL_CLAUSE")]),
        }
        after = {
            "a": record("a", "RESOLVED", availability=AVAILABLE),
            "b": record("b", "UNRESOLVED", [("SHIPPING", "SHIPPING_UNRECOGNIZED_DELIVERY_CLAUSE")]),
        }
        report = regressions(before, after)
        self.assertFalse(needs_review(report))
        self.assertEqual({("SHIPPING", "SHIPPING_CONDITIONAL_CLAUSE", "SHIPPING_UNRECOGNIZED_DELIVERY_CLAUSE"): 1},
                         dict(report["reattributed"]))

    def test_should_flag_worse_statuses_new_issues_changed_decisions_and_missing_cases(self):
        before = {
            "worse": record("worse", "RESOLVED"),
            "new": record("new", "RESOLVED"),
            "changed": record("changed", "RESOLVED", availability=AVAILABLE),
            "gone": record("gone", "RESOLVED"),
        }
        after = {
            "worse": record("worse", "INVALID", [("PRICE", "PRICE_PARSE_FAILED")]),
            "new": record("new", "UNRESOLVED", [("TITLE", "TITLE_UNKNOWN_LANGUAGE")]),
            "changed": record("changed", "RESOLVED", availability={"action": "SET", "value": "SOLD_OUT"}),
        }
        report = regressions(before, after)
        self.assertTrue(needs_review(report))
        self.assertEqual(["gone"], report["missing"])
        self.assertEqual(2, sum(report["worse"].values()))
        self.assertIn(("TITLE", "TITLE_UNKNOWN_LANGUAGE"), report["new_issues"])
        self.assertEqual({"availability": 1}, dict(report["changed_decisions"]))


class ReportingTest(unittest.TestCase):
    def test_should_compare_resolved_counts_per_recorded_issue(self):
        before = {"recordedIssues": {"PRICE/PRICE_PARSE_FAILED": {"INVALID": 10}}, "statuses": {}}
        after = {"recordedIssues": {"PRICE/PRICE_PARSE_FAILED": {"INVALID": 7, "RESOLVED": 3}}, "statuses": {}}
        self.assertEqual([("PRICE/PRICE_PARSE_FAILED", 10, 0, 3)], comparison_rows(before, after))

    def test_should_build_fixtures_in_the_shape_and_style_of_the_reviewed_file(self):
        existing = json.loads(FIXTURES.read_text(encoding="utf-8").splitlines()[0])
        spec = {"name": "example", "category": "REPAIRED", "title": "Antique oak chair",
                "description": "A restored oak chair.", "availability": "Bestellen", "fields": ["availability"]}
        case = synthetic_case(spec)
        self.assertEqual(set(existing["case"]), set(case))
        self.assertEqual(set(existing["case"]["evidence"]["input"]), set(case["evidence"]["input"]))
        self.assertEqual("", case["listingSourceId"])
        result = {"status": "RESOLVED", "decision": "RESOLVED", "issues": [], "fields": {"availability": AVAILABLE}}
        line = fixture_line(spec, result)
        self.assertIn('"name": "example"', line)
        self.assertEqual({"status", "decision", "issues", "fields"}, set(json.loads(line)["expected"]))

    def test_should_refuse_to_pin_a_field_that_an_invalid_result_does_not_write(self):
        spec = {"name": "example", "category": "NEGATIVE", "title": "Antique oak chair",
                "description": "A restored oak chair.", "price": "12,- 13,- €", "fields": ["price"]}
        result = {"status": "INVALID", "decision": "INVALID", "issues": [], "fields": {}}
        with self.assertRaises(SystemExit):
            fixture_line(spec, result)
        self.assertEqual({}, json.loads(fixture_line({**spec, "fields": []}, result))["expected"]["fields"])


class CaptureTimeTest(unittest.TestCase):
    def test_should_read_the_capture_time_from_a_uuid_v7_revision_id(self):
        self.assertEqual("2026-11-10 13:41", capture_time("01a1c5b1-3f10-7000-8000-000000000000"))


if __name__ == "__main__":
    unittest.main()
