#!/usr/bin/env python3
"""Credential-safe Aura API client and append-only dealer ledger audit."""

import argparse
from collections import Counter
import json
import os
from pathlib import Path
import stat
import sys
from urllib.error import HTTPError
from urllib.parse import urlencode, urlsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener

PRODUCTION = "https://api.aura-historia.com"
STAGING = "https://api.stage.aura-historia.com"


def canonical_host(url):
    host = urlsplit(url).hostname
    if not host:
        raise ValueError("Expected an absolute URL with a hostname")
    return host.lower().removeprefix("www.").encode("idna").decode("ascii")


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise ValueError("API redirects are refused to protect credentials")


class AuraApi:
    def __init__(self, base=PRODUCTION, credential_file=None):
        if base not in (PRODUCTION, STAGING):
            raise ValueError("Use an explicitly supported Aura API origin")
        self.base = base
        token = os.environ.get("AURA_HISTORIA_ACCESS_TOKEN", "").strip()
        if credential_file:
            path = Path(credential_file)
            info = path.stat()
            if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
                raise ValueError("Credential file must be owner-only and owned by this user")
            token = path.read_text().strip()
        if not token or any(c.isspace() for c in token):
            raise ValueError("Supply AURA_HISTORIA_ACCESS_TOKEN or an external owner-only file")
        self._token = token
        self._opener = build_opener(NoRedirect())

    def request(self, method, path, body=None):
        parts = urlsplit(path)
        if parts.scheme or parts.netloc or not parts.path.startswith("/v1/") or parts.fragment:
            raise ValueError("Expected a relative /v1/ API path")
        data = None if body is None else json.dumps(body, ensure_ascii=False).encode()
        req = Request(self.base + path, data=data, method=method, headers={
            "Authorization": "Bearer " + self._token,
            "Content-Type": "application/json", "Accept": "application/json",
        })
        try:
            with self._opener.open(req, timeout=30) as response:
                raw = response.read()
                return json.loads(raw) if raw else None
        except HTTPError as error:
            raise RuntimeError(f"Aura API returned HTTP {error.code}; reconcile before retrying writes") from None

    def collection(self, path):
        items, cursor, seen = [], None, set()
        while True:
            params = {"size": 100}
            if cursor:
                params["searchAfter"] = cursor
            page = self.request("GET", path + "?" + urlencode(params))
            items.extend(page["items"])
            cursor = page.get("searchAfter")
            if not cursor:
                return items
            if cursor in seen:
                raise ValueError("Repeated pagination cursor; snapshot is incomplete")
            seen.add(cursor)


def ledger_rows(path):
    # JSONL uses LF delimiters; str.splitlines() also splits valid JSON string content.
    with Path(path).open(encoding="utf-8") as stream:
        for number, line in enumerate(stream, 1):
            yield number, json.loads(line)


def read_ledger(path):
    latest = {}
    for number, row in ledger_rows(path):
        if row.get("schemaVersion") != 1 or canonical_host(row["url"]) != row["domain"]:
            raise ValueError(f"Invalid ledger identity at line {number}")
        if row["status"] not in {"REVIEWED", "IN_PROGRESS", "ADDED", "EXCLUDED", "DEFERRED"}:
            raise ValueError(f"Invalid status at line {number}")
        if not row.get("observedAt") or not row.get("evidence"):
            raise ValueError(f"Missing observation/evidence at line {number}")
        for field, assertion in row.get("facts", {}).items():
            if not 0.95 < assertion["confidence"] <= 1 or not assertion.get("evidence"):
                raise ValueError(f"Unqualified fact {field} at line {number}")
        state = latest.get(row["domain"], {}) | row
        if state["status"] == "ADDED":
            ids = state.get("backend", {})
            if not ids.get("partyId") or not ids.get("listingSourceId") or not state.get("verifiedAt"):
                raise ValueError(f"Incomplete added checkpoint at line {number}")
        latest[row["domain"]] = state
    aliases, source_ids = {}, {}
    for domain, row in latest.items():
        if row["status"] == "ADDED":
            if row.get("pendingOperation") or row.get("ingestion", {}).get("methods") != []:
                raise ValueError("Added dealers must have no pending write and empty ingestion methods")
        for alias in [domain, *row.get("aliases", [])]:
            if alias in aliases and aliases[alias] != domain:
                raise ValueError(f"Alias has multiple owners: {alias}")
            aliases[alias] = domain
        source = row.get("backend", {}).get("listingSourceId")
        if source and source in source_ids and source_ids[source] != domain:
            raise ValueError("Source ID has multiple canonical records; reconcile aliases")
        if source:
            source_ids[source] = domain
    return latest


def audit(path):
    latest = read_ledger(path)
    return {"uniqueDomains": len(latest), "statuses": dict(Counter(r["status"] for r in latest.values()))}


def write_index(path):
    import csv

    writer = csv.writer(sys.stdout, delimiter="\t", lineterminator="\n")
    writer.writerow(["domain", "status", "platform", "listingSourceId", "partyId", "observedAt", "reason", "aliases"])
    for domain, row in sorted(read_ledger(path).items()):
        ids = row.get("backend", {})
        platform = row.get("facts", {}).get("platformTechnology", {}).get("value", "")
        writer.writerow([domain, row["status"], platform, ids.get("listingSourceId", ""),
                         ids.get("partyId", ""), row["observedAt"], row.get("reasonCode", ""),
                         ";".join(row.get("aliases", []))])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", choices=[PRODUCTION, STAGING], default=PRODUCTION)
    parser.add_argument("--credential-file")
    sub = parser.add_subparsers(dest="command", required=True)
    request = sub.add_parser("request")
    request.add_argument("method", choices=["GET", "POST", "PUT", "PATCH"])
    request.add_argument("path")
    request.add_argument("--body", type=Path)
    sub.add_parser("snapshot")
    ledger = sub.add_parser("audit")
    ledger.add_argument("ledger", type=Path)
    index = sub.add_parser("index")
    index.add_argument("ledger", type=Path)
    args = parser.parse_args()
    if args.command == "index":
        write_index(args.ledger)
        return
    if args.command == "audit":
        result = audit(args.ledger)
    else:
        api = AuraApi(args.base, args.credential_file)
        if args.command == "snapshot":
            result = {"parties": api.collection("/v1/admin/parties"),
                      "listingSources": api.collection("/v1/admin/listing-sources")}
        else:
            result = api.request(args.method, args.path, json.loads(args.body.read_text()) if args.body else None)
    print(json.dumps(result, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
