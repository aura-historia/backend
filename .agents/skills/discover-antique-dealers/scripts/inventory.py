#!/usr/bin/env python3
"""Reconcile the single-location dealer inventory policy on source location defaults.

A DEALER source whose operator Party has exactly one active location, and whose active
default selects that location, also asserts advertised inventory there. The assertion
carries POLICY_REFERENCE so it stays distinguishable from first-party inventory evidence.
Only policy assertions are ever removed; explicit evidence is never changed.
"""

import argparse
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
import json
import sys
from urllib.parse import urlencode

from api import AuraApi

POLICY_REFERENCE = "aura-historia:policy/single-location-dealer-inventory/v1"
POLICY_EVIDENCE = {"reference": POLICY_REFERENCE, "scope": "ADVERTISED_INVENTORY"}


def active_location_ids(api, party_id):
    ids, after = [], None
    while True:
        params = {"limit": 100} | ({"after": after} if after else {})
        page = api.request("GET", "/v1/parties/" + party_id + "/locations?" + urlencode(params))
        ids.extend(item["id"] for item in page["items"] if item["lifecycle"] == "ACTIVE")
        after = page.get("next")
        if not after:
            return ids


def qualifies(source_type, location_ids, default):
    assignment = default.get("assignment")
    return (source_type == "DEALER" and len(location_ids) == 1 and assignment is not None
            and assignment["partyLocationId"] == location_ids[0]
            and default.get("eligibility") == "ACTIVE")


def decide(source_type, location_ids, default):
    """Return (action, reason); action is SET, CLEAR or None."""
    assignment = default.get("assignment")
    inventory = (assignment or {}).get("advertisedInventory")
    eligible = qualifies(source_type, location_ids, default)
    if inventory and inventory["reference"] != POLICY_REFERENCE:
        return None, "EXPLICIT_EVIDENCE"
    if inventory:
        if eligible:
            return None, "POLICY_CURRENT"
        if default.get("eligibility") != "ACTIVE":
            return None, "POLICY_ON_RETIRED_REFERENCE"
        return "CLEAR", "POLICY_NO_LONGER_APPLIES"
    if eligible:
        return "SET", "SINGLE_LOCATION_DEALER"
    if source_type != "DEALER":
        return None, "NOT_DEALER"
    if assignment is None:
        return None, "NO_DEFAULT"
    if default.get("eligibility") != "ACTIVE":
        return None, "RETIRED_DEFAULT"
    if not location_ids:
        return None, "NO_ACTIVE_LOCATION"
    if len(location_ids) > 1:
        return None, "MULTIPLE_LOCATIONS"
    return None, "DEFAULT_SELECTS_OTHER_LOCATION"


def expected_inventory_reference(api, row, ids):
    """Inventory reference a managed dealer's selected default must carry, or None."""
    if row.get("inventoryEvidence"):
        return row["inventoryEvidence"]
    selected = ids.get("partyLocationIds", [])[:1]
    if selected and active_location_ids(api, ids["partyId"]) == selected:
        return POLICY_REFERENCE
    return None


def default_path(source_id):
    return "/v1/admin/listing-sources/" + source_id + "/location-default"


def inspect(api, source, party_locations):
    source_id = source["listingSourceId"]
    default = api.request("GET", default_path(source_id))
    locations = party_locations(source["operator"]["partyId"])
    action, reason = decide(source["listingSourceType"], locations, default)
    return {"listingSourceId": source_id, "action": action, "reason": reason,
            "revision": default["revision"], "activeLocations": len(locations)}


def apply(api, plan):
    path = default_path(plan["listingSourceId"])
    evidence = POLICY_EVIDENCE if plan["action"] == "SET" else None
    api.request("PATCH", path, {"expectedRevision": plan["revision"], "advertisedInventory": evidence})
    current = api.request("GET", path)
    inventory = (current.get("assignment") or {}).get("advertisedInventory")
    if inventory != evidence:
        raise ValueError("Advertised inventory readback differs")


def main():
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--credential-file")
    parser.add_argument("--apply", action="store_true", help="Write changes; default is a read-only plan")
    parser.add_argument("--source", action="append", help="Restrict to listing source IDs; repeatable")
    parser.add_argument("--limit", type=int, help="Maximum number of writes in this run")
    parser.add_argument("--workers", type=int, default=4, help="Concurrent read workers")
    args = parser.parse_args()
    api = AuraApi(credential_file=args.credential_file)
    sources = api.collection("/v1/admin/listing-sources")
    if args.source:
        sources = [s for s in sources if s["listingSourceId"] in set(args.source)]
    cache = {}

    def party_locations(party_id):
        if party_id not in cache:
            cache[party_id] = active_location_ids(api, party_id)
        return cache[party_id]

    with ThreadPoolExecutor(max_workers=args.workers) as executor:
        plans = list(executor.map(lambda s: inspect(api, s, party_locations), sources))
    pending = [p for p in plans if p["action"]]
    written, failures = Counter(), 0
    if args.apply:
        for plan in pending[:args.limit]:
            try:
                apply(api, plan)
                written[plan["action"]] += 1
                print(plan["action"], plan["listingSourceId"], flush=True)
            except (RuntimeError, ValueError) as error:
                failures += 1
                print("STOP", plan["listingSourceId"], error, flush=True)
                if "HTTP 401" in str(error) or "HTTP 403" in str(error):
                    break
    print(json.dumps({"listingSources": len(plans),
                      "byReason": dict(Counter(p["reason"] for p in plans)),
                      "byActiveLocations": dict(Counter(min(p["activeLocations"], 2) for p in plans)),
                      "pending": dict(Counter(p["action"] for p in pending)),
                      "written": dict(written), "failures": failures}))
    return bool(failures)


if __name__ == "__main__":
    sys.exit(main())
