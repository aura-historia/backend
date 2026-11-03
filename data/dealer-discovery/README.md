# Antique dealer discovery

Continue with **“Run $discover-antique-dealers to find further dealers.”** The
[repository skill](../../.agents/skills/discover-antique-dealers/SKILL.md) owns the
workflow and [API notes](../../.agents/skills/discover-antique-dealers/references/api.md)
explain the onboarding operations. Discovery order is Germany → Central Europe →
rest of Europe → US → Canada; suitable incidental finds elsewhere are welcome.

Add independent dealers, with first-party evidence and assessed confidence above 95%
for every imported fact. Exclude marketplaces and all auction businesses. Historical
militaria alone is not disqualifying. Do not contact merchants or create partnerships.
**Leave ingestion methods empty. Identify platform technology independently**, preferring
the commerce platform over its underlying CMS. Unknown platform remains absent.

## Country indexes

All six country runs use `countries/ISO2/`:

| Country | Notes | Domain index | Evidence ledger |
| --- | --- | --- | --- |
| Germany (DE) | [Notes](countries/DE/README.md) | [Index](countries/DE/index.tsv) | [Ledger](countries/DE/dealers.jsonl) |
| Spain (ES) | [Notes](countries/ES/README.md) | [Index](countries/ES/index.tsv) | [Ledger](countries/ES/dealers.jsonl) |
| France (FR) | [Notes](countries/FR/README.md) | [Index](countries/FR/index.tsv) | [Ledger](countries/FR/dealers.jsonl) |
| United Kingdom (GB) | [Notes](countries/GB/README.md) | [Index](countries/GB/index.tsv) | [Ledger](countries/GB/dealers.jsonl) |
| Italy (IT) | [Notes](countries/IT/README.md) | [Index](countries/IT/index.tsv) | [Ledger](countries/IT/dealers.jsonl) |
| Netherlands (NL) | [Notes](countries/NL/README.md) | [Index](countries/NL/index.tsv) | [Ledger](countries/NL/dealers.jsonl) |

Each country keeps its evidence and checkpoint history in `dealers.jsonl`, queued leads
and directory refreshes in `frontier.jsonl`, a generated lookup in `index.tsv`, and live
readback results in `verification.json`. Consult country notes, search history where
present, and dated frontier observations before repeating research. Include nested
hosted-dealer ledgers in duplicate scans. None of these files contains API credentials.

## Ledger and resumption

JSONL schema version 1 is append-only. Every event has `schemaVersion`, `run`, `domain`,
`url`, `observedAt`, `status`, and evidence URLs. Canonical domains remove leading `www.`
and normalize IDNA; confirmed alternate domains are `aliases`. Do not treat subdomains
or similar names as the same business without evidence.

Fold events in file order by domain using **top-level replacement of provided fields**;
omitted fields survive. Nested objects such as `facts` and `backend` replace the whole
previous object. Keep the complete current nested object when updating it. The initial
review records hold `facts` entries with `value`, `confidence` and evidence; later events
carry IDs and operation checkpoints. Confidence is an evidence assessment, not a
calibrated probability or proof of merchant reliability.
Read JSONL as file lines: Unicode paragraph separators inside JSON strings are valid
content and must not be treated as record delimiters by `str.splitlines()`.
Distinct merchant catalogues may have separate source records under one managed Party.
Their domains remain separate from redirect aliases, and confirmed locations can be
shared. Party and location totals count unique resource IDs; source policies stay separate.

| Status | Meaning / next action |
| --- | --- |
| REVIEWED | Evidence prepared; backend creation not confirmed. |
| IN_PROGRESS | Resume from recorded IDs and reconcile any `pendingOperation` before retrying. |
| ADDED | Resources/defaults read back successfully; enrich this identity rather than recreate it. |
| EXCLUDED | Skip while the recorded reason holds. Revisit only with new evidence or relevant scope changes. |
| DEFERRED | No backend creation; revisit after the stated date or when missing evidence becomes available. |

Use one ledger writer at a time. Record intent before a write and returned IDs
immediately afterward. Shipping profile creation keys are source-scoped and stable;
Party/source/location POSTs need reconciliation after an uncertain response. Names and
slug suffixes do not prevent duplicates. Check both ledger aliases and live admin
Party/source searches before creating anything. Before reporting completion, confirm
Party ownership, locations, source presentation, empty ingestion lists, platform values,
active delivery profiles and explicitly selected defaults.

```sh
python3 .agents/skills/discover-antique-dealers/scripts/api.py audit data/dealer-discovery/countries/DE/dealers.jsonl
python3 .agents/skills/discover-antique-dealers/scripts/api.py index data/dealer-discovery/countries/DE/dealers.jsonl > data/dealer-discovery/countries/DE/index.tsv
```

Reusable [onboarding](../../.agents/skills/discover-antique-dealers/scripts/onboard.py)
and [verification](../../.agents/skills/discover-antique-dealers/scripts/verify.py)
commands now consume this ledger directly. Select a run explicitly, inspect its
`--dry-run`, then onboard it. The verifier writes `verification.json` only after all checks pass. Its default is
a full live readback; `--incremental` checks all Party/source metadata and changed
observations while preserving earlier location/profile verification dates. See the
skill for command syntax and periodic full-check guidance.
Neither command retries uncertain writes or enables ingestion.

## Parallel country runs

Country agents use `countries/ISO2/{dealers.jsonl,frontier.jsonl,index.tsv,verification.json}`
and their own notes. Assign new dealers by their verified business home country, not
their domain suffix or shipping destinations. Read every country ledger, including
nested hosted-dealer ledgers, before claiming a dealer, then perform fresh API duplicate
checks.

Each operator has one ledger owner. Germany's ledger retains its earlier foreign
dealers until an explicit handoff; do not copy them into a second ledger or mutate
another owner's API resources. Historical audit records that name the former
`data/dealer-discovery/dealers.jsonl` refer to the history now held in
`countries/DE/dealers.jsonl`. Queue foreign leads or proposed enrichment in your own
frontier with the target country/owner. Do not edit another agent's files or shared
skill/scripts. Use one writer per ledger.

A separately incorporated foreign business can share a website with a German dealer.
Queue it as `recordType: FOREIGN_ENTITY_LEAD` with a stable `entityKey` such as country
and registration number. Keep that queue entry separate from domain decisions; a shared
website alone establishes neither shared legal ownership nor another catalogue source.

Pass `--ledger` to onboarding and verification, and also `--output` to verification;
their defaults point at Germany's `countries/DE/` files. Pass the country ledger
explicitly to `api.py audit` and `api.py index`, writing the index into that country's
directory. Country checkpoints describe only that ledger, not the platform-wide total.
Country agents use free tools; the original paid-data budget is shared, not per country.

The API helper uses `AURA_HISTORIA_ACCESS_TOKEN` or an external owner-only credential
file; credentials are not part of this memory. It refuses redirects and arbitrary API
origins and never automatically retries writes. Cache raw pages/snapshots outside the
repository and retain concise evidence here. Keep the generated index synchronized.

The credential-free [public fetcher](../../.agents/skills/discover-antique-dealers/scripts/fetch.py)
uses bounded requests, external caches and JSONL previews. Full cache JSON retains main
content, original image attributes, resolved links and encoding observations. Technology
hints require manual verification. It does not write business records or spend paid-data credit.
