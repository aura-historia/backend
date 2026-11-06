"""Exercise shared-operator onboarding with a stateful local API fixture; no network."""

import copy
from contextlib import redirect_stdout
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from urllib.parse import parse_qs, urlsplit

from api import read_ledger
from onboard import onboard
import verify


def observation(domain, status):
    url = "https://" + domain + "/"
    def fact(value):
        return {"value": value, "confidence": .99, "evidence": [url]}
    location = {"label": "Office", "roles": ["REGISTERED_ADDRESS"],
                "geography": {"referenceRelease": "iso-codes-4.20.1", "country": "DE",
                              "addressText": "Berlin, Deutschland"},
                "evidence": {"reference": url, "scope": "REGISTERED_ADDRESS"}}
    return {"schemaVersion": 1, "run": "fixture", "domain": domain, "url": url,
            "observedAt": "2026-11-02T00:00:00Z", "status": status, "evidence": [url],
            "facts": {"name": fact(domain), "operatorName": fact("Shared dealer"),
                      "email": fact("dealer@example.org"), "phone": fact("+49301234"),
                      "platformTechnology": fact("SHOPWARE"), "locations": fact([location])},
            "qualification": {"confidence": .98}, "ingestion": {"methods": []}, "backend": {}}


class StatefulApi:
    def __init__(self, owner):
        self.requests = []
        self.fail_after_source = False
        self.parties = {"pty_owner": {"partyId": "pty_owner", "name": "Shared dealer",
                                      "contact": {"email": "dealer@example.org", "phone": "+49301234"}}}
        self.locations = {"ploc_owner": {"id": "ploc_owner", "partyId": "pty_owner",
                                        "lifecycle": "ACTIVE", **copy.deepcopy(owner["facts"]["locations"]["value"][0])}}
        self.sources = {"ls_owner": self.source(owner["url"], owner["domain"], "pty_owner", "SHOPWARE")}
        self.defaults = {"/v1/admin/listing-sources/ls_owner/location-default": {
            "revision": 2, "eligibility": "ACTIVE", "assignment": {"partyLocationId": "ploc_owner"}}}

    @staticmethod
    def source(url, name, party, platform):
        return {"url": url, "name": name, "operator": {"partyId": party},
                "platformTechnology": platform, "ingestionMethods": []}

    def request(self, method, path, body=None):
        self.requests.append((method, path, copy.deepcopy(body)))
        route = urlsplit(path).path
        if method == "GET" and route == "/v1/admin/parties":
            query = parse_qs(urlsplit(path).query)
            field = next(k for k in query if k != "size")
            items = [p for p in self.parties.values() if query[field][0] in
                     (p["name"] if field == "name" else p["contact"].get(field, ""))]
            return {"items": copy.deepcopy(items)}
        if method == "POST" and route == "/v1/admin/listing-sources":
            operator = body["operator"]
            if operator["type"] == "EXISTING":
                party = operator["partyId"]
                assert party in self.parties
            else:
                party = "pty_new"
                self.parties[party] = {"partyId": party, "name": operator["name"],
                                       "contact": {k: operator[k] for k in ("email", "phone") if k in operator}}
            self.sources["ls_second"] = self.source(body["url"], body["name"], party,
                                                     body.get("platformTechnology"))
            assert body["ingestionConfiguration"] == []
            assert body["listingSourceType"] == "DEALER"
            if self.fail_after_source:
                raise RuntimeError("Fixture lost response after source creation")
            return {"listingSourceId": "ls_second", "partyId": party}
        if route.endswith("/location-default"):
            if method == "PUT":
                self.defaults[route] = {"revision": 2, "eligibility": "ACTIVE",
                                         "assignment": {"partyLocationId": body["partyLocationId"],
                                                        "businessAssociation": body["businessAssociation"]}}
            return copy.deepcopy(self.defaults.get(route, {"revision": 1, "assignment": None}))
        if method == "GET" and route.endswith("/shipping-default"):
            return {"revision": 1, "shippingProfileId": None}
        if method == "GET" and "/locations/" in route:
            return copy.deepcopy(self.locations[route.rsplit("/", 1)[1]])
        if method == "GET" and route.startswith("/v1/admin/parties/"):
            return copy.deepcopy(self.parties[route.rsplit("/", 1)[1]])
        if method == "GET" and route.startswith("/v1/admin/listing-sources/"):
            return copy.deepcopy(self.sources[route.rsplit("/", 1)[1]])
        raise AssertionError("Unexpected API request: " + method + " " + path)

    def writes(self):
        return [r for r in self.requests if r[0] != "GET"]

    def collection(self, path):
        if path == "/v1/admin/parties":
            return copy.deepcopy(list(self.parties.values()))
        if path == "/v1/admin/listing-sources":
            return [copy.deepcopy(source) | {"listingSourceId": source_id,
                    "presentation": {"url": source["url"], "image": source.get("image")}}
                    for source_id, source in self.sources.items()]
        raise AssertionError("Unexpected snapshot: " + path)


class OperatorReuseTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.ledger = Path(self.temporary.name) / "dealers.jsonl"
        self.owner = observation("first.example.org", "ADDED")
        self.owner.update(verifiedAt="2026-11-02T00:00:00Z",
                          backend={"partyId": "pty_owner", "listingSourceId": "ls_owner",
                                   "partyLocationIds": ["ploc_owner"]})
        self.row = observation("second.example.org", "REVIEWED")
        self.row["existingOperator"] = {"ownerDomain": self.owner["domain"], "partyId": "pty_owner",
                                        "partyLocationIds": ["ploc_owner"], "confidence": .99,
                                        "evidence": [self.owner["url"], self.row["url"]]}
        self.save()
        self.api = StatefulApi(self.owner)

    def save(self):
        self.ledger.write_text("".join(json.dumps(r) + "\n" for r in [self.owner, self.row]))

    def run_onboard(self, dry=False, row=None):
        with redirect_stdout(io.StringIO()):
            onboard(self.api, self.ledger, row or self.row, [], dry)

    def test_should_create_second_source_without_duplicate_party_or_location(self):
        self.run_onboard()
        state = read_ledger(self.ledger)
        child = state[self.row["domain"]]
        self.assertEqual("ADDED", child["status"])
        self.assertEqual("pty_owner", child["backend"]["partyId"])
        self.assertEqual(["ploc_owner"], child["backend"]["partyLocationIds"])
        self.assertEqual((1, 2, 1), (len(self.api.parties), len(self.api.sources), len(self.api.locations)))
        self.assertEqual(self.owner, state[self.owner["domain"]])
        self.assertEqual(["POST", "PUT"], [r[0] for r in self.api.writes()])
        self.assertEqual({"type": "EXISTING", "partyId": "pty_owner"},
                         self.api.writes()[0][2]["operator"])

    def test_should_leave_api_and_ledger_unchanged_during_dry_run(self):
        before = self.ledger.read_bytes()
        self.run_onboard(dry=True)
        self.assertEqual([], self.api.writes())
        self.assertEqual(before, self.ledger.read_bytes())

    def test_should_count_unique_resources_after_full_shared_operator_verification(self):
        self.run_onboard()
        output = self.ledger.parent / "verification.json"
        args = ["verify.py", "--run", "fixture", "--ledger", str(self.ledger),
                "--output", str(output), "--paid-data-spend-usd", "0"]
        with patch.object(verify, "AuraApi", return_value=self.api), patch("sys.argv", args):
            with redirect_stdout(io.StringIO()):
                verify.main()
        result = json.loads(output.read_text())
        self.assertEqual("FULL", result["scope"])
        self.assertEqual(1, result["counts"]["parties"])
        self.assertEqual(2, result["counts"]["listingSources"])
        self.assertEqual(1, result["counts"]["locations"])
        self.assertEqual(2, result["counts"]["emptyIngestionMethods"])

    def test_should_stop_when_declared_owner_is_outside_its_ledger(self):
        self.row["existingOperator"]["ownerDomain"] = "unmanaged.example.org"
        with self.assertRaisesRegex(ValueError, "managed owner"):
            self.run_onboard()
        self.assertEqual([], self.api.writes())

    def test_should_stop_when_live_party_contact_changed(self):
        self.api.parties["pty_owner"]["contact"]["email"] = "changed@example.org"
        with self.assertRaisesRegex(ValueError, "Party contact differs"):
            self.run_onboard()
        self.assertEqual([], self.api.writes())

    def test_should_stop_when_location_has_another_live_owner(self):
        self.api.locations["ploc_owner"]["partyId"] = "pty_other"
        with self.assertRaisesRegex(ValueError, "Location owner"):
            self.run_onboard()
        self.assertEqual([], self.api.writes())

    def test_should_journal_uncertain_post_and_refuse_automatic_retry(self):
        self.api.fail_after_source = True
        with self.assertRaises(RuntimeError):
            self.run_onboard()
        current = read_ledger(self.ledger)[self.row["domain"]]
        self.assertEqual("POST", current["pendingOperation"]["method"])
        writes = len(self.api.writes())
        with self.assertRaisesRegex(ValueError, "Uncertain previous operation"):
            self.run_onboard(row=current)
        self.assertEqual(writes, len(self.api.writes()))
        self.assertEqual((1, 2, 1), (len(self.api.parties), len(self.api.sources), len(self.api.locations)))

    def test_should_preserve_new_operator_creation_for_new_dealer(self):
        self.row.pop("existingOperator")
        for key, value in [("operatorName", "Another dealer"), ("email", "other@example.org"),
                           ("phone", "+49305678"), ("locations", [])]:
            self.row["facts"][key]["value"] = value
        self.save()
        self.run_onboard()
        self.assertEqual("NEW", self.api.writes()[0][2]["operator"]["type"])
        self.assertEqual(2, len(self.api.parties))


if __name__ == "__main__":
    unittest.main()
