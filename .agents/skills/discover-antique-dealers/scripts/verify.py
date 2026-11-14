#!/usr/bin/env python3
"""Verify managed dealers and save a dated full or incremental checkpoint."""

import argparse
from concurrent.futures import ThreadPoolExecutor
import copy
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path

from api import AuraApi, PRODUCTION, read_ledger
from inventory import expected_inventory_reference

ROOT = Path(__file__).resolve().parents[4]
LEDGER = ROOT / "data/dealer-discovery/countries/DE/dealers.jsonl"


def now():
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


def require(condition, message):
    if not condition:
        raise ValueError(message)


def value(row, field, default=None):
    return row["facts"].get(field, {}).get("value", default)


def check_identity(api, row, ids):
    source = api.request("GET", "/v1/admin/listing-sources/" + ids["listingSourceId"])
    party = api.request("GET", "/v1/admin/parties/" + ids["partyId"])
    check_identity_fields(source, party, row, ids)
    return source


def check_identity_fields(source, party, row, ids):
    require(source["operator"]["partyId"] == ids["partyId"], "Source operator differs")
    check_party_fields(party, row)
    require(source["url"] == row["url"] and source["name"] == value(row, "name"),
            "Source presentation differs")
    require(source["ingestionMethods"] == [], "Ingestion methods must remain empty")
    require(source.get("platformTechnology") == value(row, "platformTechnology"),
            "Platform technology differs")
    require(source.get("image") == value(row, "logo", {}).get("url"), "Source image differs")


def check_party_fields(party, row):
    require(party["name"] == value(row, "operatorName"), "Party name differs")
    for field in ("email", "phone"):
        require(party.get("contact", {}).get(field) == value(row, field),
                "Party contact differs: " + field)


def observation_digest(row):
    fields = ("url", "facts", "backend", "ingestion", "inventoryEvidence", "collectionSiteIndex")
    raw = json.dumps({key: row.get(key) for key in fields}, sort_keys=True, ensure_ascii=False)
    return hashlib.sha256(raw.encode()).hexdigest()


def check_location(api, row, ids, index):
    location = api.request("GET", "/v1/parties/" + ids["partyId"] + "/locations/"
                           + ids["partyLocationIds"][index])
    expected = value(row, "locations", [])[index]
    require(location["partyId"] == ids["partyId"] and location["lifecycle"] == "ACTIVE",
            "Location owner or lifecycle differs")
    require(location["geography"] == expected["geography"], "Location geography differs")
    require(set(location["roles"]) == set(expected["roles"])
            and len(location["roles"]) == len(expected["roles"]), "Location roles differ")


def check_location_default(current, ids, inventory_reference):
    expected = ids.get("partyLocationIds", [])
    assignment = current.get("assignment")
    if not expected:
        require(assignment is None, "Unexpected business location default")
        return
    require(assignment and assignment["partyLocationId"] == expected[0]
            and current["eligibility"] == "ACTIVE", "Business location default differs")
    require((assignment.get("advertisedInventory") or {}).get("reference") == inventory_reference,
            "Advertised inventory assertion differs")


def shipping_terms(row, ids):
    terms = copy.deepcopy(value(row, "shippingTerms"))
    index = row.get("collectionSiteIndex")
    # These three initial observations predate the explicit index field.
    if index is None and row["domain"] in {"antik-heymann.de", "antik-im-hof.de", "vintage-antik.shop"}:
        index = 0
    if terms and index is not None:
        terms["collection"]["site"] = {
            "kind": "PARTY_LOCATION", "party_location_id": ids["partyLocationIds"][index]}
    return terms


def normalized_shipping_terms(terms):
    normalized = copy.deepcopy(terms)
    shipping = normalized.get("shipping")
    if shipping and shipping["kind"] == "COVERAGE":
        for zone in shipping["coverage"]["zones"]:
            for service in zone["services"]:
                features = service["features"]
                require(isinstance(features, list)
                        and all(isinstance(feature, str) for feature in features)
                        and len(features) == len(set(features)), "Invalid delivery feature set")
                # shipping-core persists features as a set; geographic zones stay ordered.
                service["features"] = sorted(features)
    return normalized


def check_profile(api, row, ids):
    profile = api.request("GET", "/v1/listing-sources/" + ids["listingSourceId"]
                          + "/shipping-profiles/" + ids["shippingProfileId"])
    require(profile["lifecycle"] == "ACTIVE", "Delivery profile is not active")
    require(profile["content"]["attributedPartyId"] == ids["partyId"],
            "Delivery profile attribution differs")
    require(normalized_shipping_terms(profile["content"]["terms"])
            == normalized_shipping_terms(shipping_terms(row, ids)), "Delivery terms differ")


def check_shipping_default(current, ids):
    require(current.get("shippingProfileId") == ids.get("shippingProfileId"),
            "Shipping default differs")
    if ids.get("shippingProfileId"):
        require(current["eligibility"] == "ACTIVE", "Shipping default is not active")


def verify_dealer(api, row):
    ids = row["backend"]
    source = check_identity(api, row, ids)
    locations = ids.get("partyLocationIds", [])
    require(len(locations) == len(value(row, "locations", [])), "Incomplete location IDs")
    for index in range(len(locations)):
        check_location(api, row, ids, index)
    check_location_default(api.request("GET", "/v1/admin/listing-sources/"
                                      + ids["listingSourceId"] + "/location-default"),
                           ids, expected_inventory_reference(api, row, ids))
    default = api.request("GET", "/v1/listing-sources/" + ids["listingSourceId"] + "/shipping-default")
    require(bool(ids.get("shippingProfileId")) == bool(value(row, "shippingTerms")),
            "Delivery observation and profile presence differ")
    if ids.get("shippingProfileId"):
        check_profile(api, row, ids)
    check_shipping_default(default, ids)
    return {"domain": row["domain"], "listingSourceId": ids["listingSourceId"],
            "partyId": ids["partyId"], "locations": len(locations),
            "logo": bool(source.get("image")), "platform": source.get("platformTechnology"),
            "deliveryProfile": bool(ids.get("shippingProfileId")), "ingestionMethods": [],
            "verified": True, "verifiedAt": now(), "observationDigest": observation_digest(row)}


def verify_snapshots(added, parties, sources):
    party_by_id = {p["partyId"]: p for p in parties}
    source_by_id = {s["listingSourceId"]: s for s in sources}
    for row in added:
        ids = row["backend"]
        require(ids["partyId"] in party_by_id, "Managed Party missing from snapshot")
        require(ids["listingSourceId"] in source_by_id, "Managed source missing from snapshot")
        summary = source_by_id[ids["listingSourceId"]]
        source = summary | {"url": summary["presentation"].get("url"),
                            "image": summary["presentation"].get("image")}
        check_identity_fields(source, party_by_id[ids["partyId"]], row, ids)


def verify_with_baseline(api, added, baseline):
    cached = {r["domain"]: r for r in baseline.get("dealers", [])}
    changed = [r for r in added if cached.get(r["domain"], {}).get("observationDigest")
               != observation_digest(r)]
    with ThreadPoolExecutor(max_workers=3) as executor:
        fresh = list(executor.map(lambda row: verify_dealer(api, row), changed))
    cached.update({r["domain"]: r for r in fresh})
    results = [cached[r["domain"]] for r in added]
    require(all(r.get("verified") and r.get("verifiedAt") for r in results),
            "Baseline contains incomplete verification records")
    return results, len(fresh)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run", required=True, help="Checkpoint label")
    parser.add_argument("--ledger", type=Path, default=LEDGER)
    parser.add_argument("--output", type=Path, default=LEDGER.parent / "verification.json")
    parser.add_argument("--credential-file")
    parser.add_argument("--incremental", action="store_true",
                        help="Reuse unchanged location/profile checks; keep their original verification dates")
    parser.add_argument("--paid-data-spend-usd", type=float, required=True,
                        help="Actual cumulative paid data spend, explicitly supplied")
    args = parser.parse_args()
    require(args.paid_data_spend_usd >= 0, "Spend cannot be negative")
    ledger = read_ledger(args.ledger)
    api = AuraApi(credential_file=args.credential_file)
    parties = api.collection("/v1/admin/parties")
    sources = api.collection("/v1/admin/listing-sources")
    added = [r for r in ledger.values() if r["status"] == "ADDED"]
    verify_snapshots(added, parties, sources)
    baseline = {}
    if args.incremental:
        baseline = json.loads(args.output.read_text(encoding="utf-8"))
        require(baseline["environment"] == PRODUCTION, "Baseline environment differs")
    results, checked = verify_with_baseline(api, added, baseline)
    counts = {"parties": len({r["partyId"] for r in results}), "listingSources": len(results),
              "locations": len({location for row in added
                                for location in row["backend"].get("partyLocationIds", [])}),
              "logos": sum(r["logo"] for r in results),
              "identifiedPlatforms": sum(bool(r["platform"]) for r in results),
              "deliveryProfiles": sum(r["deliveryProfile"] for r in results),
              "emptyIngestionMethods": len(results),
              "excluded": sum(r["status"] == "EXCLUDED" for r in ledger.values()),
              "deferred": sum(r["status"] == "DEFERRED" for r in ledger.values())}
    timestamp = now()
    summary = {"run": args.run, "verifiedAt": timestamp,
               "scope": "INCREMENTAL" if args.incremental else "FULL", "environment": PRODUCTION,
               "fullVerifiedAt": baseline.get("fullVerifiedAt", baseline.get("verifiedAt"))
               if args.incremental else timestamp,
               "dealersCheckedThisRun": checked, "dealersReused": len(results) - checked,
               "counts": counts, "environmentCounts": {"parties": len(parties), "listingSources": len(sources)},
               "paidDataSpendUsd": args.paid_data_spend_usd,
               "checks": ["Live managed Party contacts and ownership match reviewed evidence",
                          "Location/default/profile readbacks dated individually; unchanged observations may reuse prior checks",
                          "Managed ingestion lists empty; platform metadata independent",
                          "Profile terms and active defaults match reviewed observations",
                          "Inventory inheritance has explicit evidence or the single-location dealer policy"],
               "dealers": results}
    temporary = args.output.with_suffix(".json.tmp")
    temporary.write_text(json.dumps(summary, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    temporary.replace(args.output)
    print(json.dumps(counts))


if __name__ == "__main__":
    main()
