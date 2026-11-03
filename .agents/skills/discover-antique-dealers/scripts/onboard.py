#!/usr/bin/env python3
"""Onboard reviewed dealer observations with journaled IDs and live readbacks."""

import argparse
import copy
import fcntl
import json
import os
from pathlib import Path
import sys
from urllib.parse import urlencode, urlsplit

from api import AuraApi, canonical_host, read_ledger
from verify import (LEDGER, check_identity, check_location, check_location_default, check_party_fields,
                    check_profile, check_shipping_default, now, require, shipping_terms,
                    value, verify_dealer)

CREATION_KEY = "dealer-discovery-2026-11-01-v1"


def checkpoint(ledger, row, ids, status="IN_PROGRESS", **extra):
    event = {key: row[key] for key in ("schemaVersion", "run", "domain", "url", "evidence")}
    event.update(observedAt=now(), status=status, backend=ids, **extra)
    with ledger.open("a", encoding="utf-8") as stream:
        stream.write(json.dumps(event, ensure_ascii=False, separators=(",", ":")) + "\n")
        stream.flush()
        os.fsync(stream.fileno())


def validate_observation(row):
    require(row["status"] in {"REVIEWED", "IN_PROGRESS", "ADDED"}, "Dealer is not reviewed")
    require(not row.get("pendingOperation"), "Uncertain previous operation; reconcile before resuming")
    require(row.get("ingestion", {}).get("methods") == [], "Ingestion methods must remain empty")
    require(row.get("qualification", {}).get("confidence", 0) > .95, "Qualification confidence missing")
    require(value(row, "name") and value(row, "operatorName"), "Reviewed business names required")
    if value(row, "logo"):
        logo = value(row, "logo")
        parts = urlsplit(logo["url"])
        require(parts.scheme in {"https", "http"} and parts.hostname and not parts.username
                and not parts.password and not parts.path.lower().endswith(".ico")
                and logo.get("type") not in {"image/x-icon", "image/vnd.microsoft.icon"},
                "Expected an inspected original HTTP logo, never ICO")


def source_matches(sources, row):
    aliases = {row["domain"], *row.get("aliases", [])}
    matches = []
    for source in sources:
        url = source.get("url") or source.get("presentation", {}).get("url")
        if url and canonical_host(url) in aliases:
            matches.append(source)
    known = row.get("backend", {}).get("listingSourceId")
    require(all(s["listingSourceId"] == known for s in matches),
            "Matching source requires explicit reconciliation")


def party_matches(api, row):
    for field in ("name", "email", "phone"):
        query = value(row, "operatorName" if field == "name" else field)
        if not query:
            continue
        page = api.request("GET", "/v1/admin/parties?" + urlencode({field: query, "size": 100}))
        require(not page.get("searchAfter"), "Party identity search incomplete; reconcile pagination")
        unresolved = [p for p in page["items"] if not (
            field == "name" and p["partyId"] in row.get("reconciledDistinctPartyIds", []))]
        require(not unresolved, "Matching Party requires explicit reconciliation")


def prepare_existing_operator(api, ledger, row, ids):
    reuse = row["existingOperator"]
    require(reuse.get("confidence", 0) > .95 and reuse.get("evidence"),
            "Existing operator requires reviewed identity evidence")
    owner = read_ledger(ledger).get(reuse.get("ownerDomain"), {})
    require(owner.get("status") == "ADDED" and owner.get("backend", {}).get("partyId")
            == reuse.get("partyId"), "Existing operator must belong to this ledger's managed owner")
    party_id = reuse["partyId"]
    require(ids.get("partyId") in {None, party_id}, "Recorded operator differs from reviewed reuse")
    check_party_fields(api.request("GET", "/v1/admin/parties/" + party_id), row)
    locations = reuse.get("partyLocationIds", [])
    require(isinstance(locations, list) and all(isinstance(i, str) for i in locations)
            and len(locations) == len(set(locations)), "Invalid reused location IDs")
    require(set(locations) <= set(owner["backend"].get("partyLocationIds", []))
            and len(locations) <= len(value(row, "locations", [])),
            "Reused locations must be reviewed locations of the managed owner")
    known = ids.get("partyLocationIds", [])
    require(not known or known[:len(locations)] == locations,
            "Recorded locations differ from reviewed reuse")
    ids["partyId"] = party_id
    if not known:
        ids["partyLocationIds"] = list(locations)
    for index in range(len(locations)):
        check_location(api, row, ids, index)


def create_source(api, ledger, row, ids, sources):
    if row.get("existingOperator"):
        operator = {"type": "EXISTING", "partyId": ids["partyId"]}
    else:
        operator = {"type": "NEW", "name": value(row, "operatorName")}
        for field in ("phone", "email"):
            if value(row, field):
                operator[field] = value(row, field)
    body = {"name": value(row, "name"), "operator": operator,
            "url": row["url"], "ingestionConfiguration": []}
    if value(row, "platformTechnology"):
        body["platformTechnology"] = value(row, "platformTechnology")
    if value(row, "logo"):
        body["image"] = value(row, "logo")["url"]
    path = "/v1/admin/listing-sources"
    checkpoint(ledger, row, ids, pendingOperation={"method": "POST", "path": path})
    response = api.request("POST", path, body)
    ids.update({key: response[key] for key in ("listingSourceId", "partyId") if response.get(key)})
    if not ids.get("listingSourceId"):
        checkpoint(ledger, row, ids, pendingOperation={"method": "POST", "path": path})
    require(ids.get("listingSourceId"), "Source write returned no ID; reconcile pending operation")
    checkpoint(ledger, row, ids, pendingOperation=None)
    if not ids.get("partyId"):
        detail = api.request("GET", path + "/" + ids["listingSourceId"])
        ids["partyId"] = detail["operator"]["partyId"]
        checkpoint(ledger, row, ids, pendingOperation=None)
    sources.append({"listingSourceId": ids["listingSourceId"], "url": row["url"]})


def ensure_locations(api, ledger, row, ids):
    known = ids.setdefault("partyLocationIds", [])
    locations = value(row, "locations", [])
    require(len(known) <= len(locations), "Unexpected extra recorded location IDs")
    for index in range(len(known)):
        check_location(api, row, ids, index)
    path = "/v1/parties/" + ids["partyId"] + "/locations"
    for index in range(len(known), len(locations)):
        checkpoint(ledger, row, ids, pendingOperation={"method": "POST", "path": path,
                                                     "locationIndex": index})
        created = api.request("POST", path, locations[index])
        known.append(created["id"])
        checkpoint(ledger, row, ids, pendingOperation=None)
        check_location(api, row, ids, index)


def ensure_location_default(api, ledger, row, ids):
    path = "/v1/admin/listing-sources/" + ids["listingSourceId"] + "/location-default"
    current = api.request("GET", path)
    if ids["partyLocationIds"] and current.get("assignment") is None:
        location = value(row, "locations")[0]
        body = {"expectedRevision": current["revision"], "partyLocationId": ids["partyLocationIds"][0],
                "businessAssociation": {"reference": location["evidence"]["reference"],
                                        "scope": "ASSOCIATED_BUSINESS"}}
        if row.get("inventoryEvidence"):
            body["advertisedInventory"] = {"reference": row["inventoryEvidence"],
                                           "scope": "ADVERTISED_INVENTORY"}
        checkpoint(ledger, row, ids, pendingOperation={"method": "PUT", "path": path})
        api.request("PUT", path, body)
        checkpoint(ledger, row, ids, pendingOperation=None)
        current = api.request("GET", path)
    check_location_default(current, row, ids)
    ids["locationDefaultRevision"] = current["revision"]


def ensure_shipping(api, ledger, row, ids):
    base = "/v1/listing-sources/" + ids["listingSourceId"]
    current = api.request("GET", base + "/shipping-default")
    require(current["shippingProfileId"] in {None, ids.get("shippingProfileId")},
            "Existing shipping default requires reconciliation")
    if not value(row, "shippingTerms"):
        require(not ids.get("shippingProfileId"), "Unexpected recorded delivery profile")
        check_shipping_default(current, ids)
        return
    if not ids.get("shippingProfileId"):
        key = ids.get("shippingCreationKey", CREATION_KEY)
        body = {"creationKey": key, "content": {"name": "Advertised dealer delivery terms",
                "attributedPartyId": ids["partyId"], "terms": shipping_terms(row, ids)},
                "evidence": [{"reference": reference, "assertion":
                              "First-party advertised shipping and collection terms; only supported scope imported."}
                             for reference in row["facts"]["shippingTerms"]["evidence"]]}
        path = base + "/shipping-profiles"
        checkpoint(ledger, row, ids, pendingOperation={"method": "POST", "path": path, "creationKey": key})
        created = api.request("POST", path, body)
        ids.update(shippingProfileId=created["shippingProfileId"], shippingCreationKey=key)
        checkpoint(ledger, row, ids, pendingOperation=None)
    check_profile(api, row, ids)
    path = base + "/shipping-default"
    if current["shippingProfileId"] is None:
        checkpoint(ledger, row, ids, pendingOperation={"method": "PUT", "path": path})
        api.request("PUT", path, {"expectedRevision": current["revision"],
                                  "shippingProfileId": ids["shippingProfileId"]})
        checkpoint(ledger, row, ids, pendingOperation=None)
        current = api.request("GET", path)
    check_shipping_default(current, ids)
    ids["shippingDefaultRevision"] = current["revision"]


def onboard(api, ledger, row, sources, dry_run):
    validate_observation(row)
    if row["status"] == "ADDED":
        print("SKIP added", row["domain"], flush=True)
        return
    ids = copy.deepcopy(row.get("backend", {}))
    if row.get("existingOperator"):
        prepare_existing_operator(api, ledger, row, ids)
    else:
        require(not ids.get("partyId") or ids.get("listingSourceId"),
                "Party without a recorded source requires explicit reconciliation")
    source_matches(sources, row)
    if ids.get("listingSourceId"):
        if not ids.get("partyId"):
            detail = api.request("GET", "/v1/admin/listing-sources/" + ids["listingSourceId"])
            ids["partyId"] = detail["operator"]["partyId"]
        check_identity(api, row, ids)
    elif not row.get("existingOperator"):
        party_matches(api, row)
    if dry_run:
        print("PLAN", row["domain"], json.dumps({"resume": bool(ids),
              "locations": len(value(row, "locations", [])), "platform": value(row, "platformTechnology"),
              "deliveryProfile": bool(value(row, "shippingTerms")), "ingestionMethods": [],
              "existingOperatorPartyId": ids.get("partyId") if row.get("existingOperator") else None}),
              flush=True)
        return
    if not ids.get("listingSourceId"):
        create_source(api, ledger, row, ids, sources)
    if row.get("existingOperator"):
        require(ids["partyId"] == row["existingOperator"]["partyId"],
                "Created source did not retain the reviewed existing operator")
    check_identity(api, row, ids)
    ensure_locations(api, ledger, row, ids)
    ensure_location_default(api, ledger, row, ids)
    ensure_shipping(api, ledger, row, ids)
    verify_dealer(api, row | {"backend": ids})
    checkpoint(ledger, row, ids, status="ADDED", verifiedAt=now(), pendingOperation=None)
    print("ADDED", row["domain"], json.dumps(ids), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    selection = parser.add_mutually_exclusive_group(required=True)
    selection.add_argument("--run")
    selection.add_argument("--domain", action="append", help="Exact canonical domain; repeatable")
    parser.add_argument("--ledger", type=Path, default=LEDGER)
    parser.add_argument("--credential-file")
    parser.add_argument("--dry-run", action="store_true", help="Live identity checks; no API or journal writes")
    args = parser.parse_args()
    # All onboarding processes must use the same ledger inode; other writers must wait.
    with args.ledger.open("r", encoding="utf-8") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        ledger = read_ledger(args.ledger)
        selected = [row for row in ledger.values() if (
            row["run"] == args.run if args.run else row["domain"] in args.domain)]
        require(selected, "No ledger rows match selection")
        if args.domain:
            require(set(args.domain) <= {r["domain"] for r in selected}, "Unknown selected domain")
        selected = [row for row in selected if row["status"] in {"REVIEWED", "IN_PROGRESS", "ADDED"}]
        require(selected, "Selection has no reviewed dealers")
        api = AuraApi(credential_file=args.credential_file)
        sources = api.collection("/v1/admin/listing-sources")
        failures = 0
        for row in selected:
            try:
                onboard(api, args.ledger, row, sources, args.dry_run)
            except Exception as error:
                failures += 1
                # Public observation payloads and API response bodies are deliberately omitted.
                message = str(error) if isinstance(error, (ValueError, RuntimeError)) else "Inspect recorded IDs before resuming"
                print("STOP", row["domain"], type(error).__name__, message, flush=True)
                if "HTTP 401" in message or "HTTP 403" in message:
                    break
        return bool(failures)


if __name__ == "__main__":
    sys.exit(main())
