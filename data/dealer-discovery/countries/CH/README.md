# Switzerland dealer discovery

Owner: Switzerland (CH), continuous discovery started 2026-11-03. Research and API onboarding are authorized until stopped. Write only this directory and its managed identities.

Country assignment uses verified business home country. CH is Switzerland’s ISO 3166-1 alpha-2 code. All other ledgers, including nested hosted ledgers, are read for duplicates. Earlier Swiss records in Germany’s ledger remain owned there.

Evidence and API checkpoints: `dealers.jsonl`; queued leads, exclusions and dated follow-ups: `frontier.jsonl`; free search coverage: `searches.jsonl`; generated lookup: `index.tsv`; explicit live readback: `verification.json`. Raw evidence and API snapshots are under `/tmp/aura-dealer-discovery/CH/`. Credentials remain external.

Only first-party facts assessed strictly above 95% confidence are imported. Auctions and marketplaces are excluded. Ingestion stays empty. Addresses, premises, stock, dispatch and collection are reviewed separately. Unknown shipping remains unknown.

Research targets must have direct art and antiques relevance, demonstrated by their own specific art, antique or original vintage-design stock. General secondhand goods, recent fashion, restoration services and directory categories alone do not qualify a dealer.

## Coverage

All 26 cantons remain open. Regional research entries and older dealer/profile leads are tracked in `frontier.jsonl`. VSAK, VSAR, VSBN and regional directories supply leads; sampled dealers and completed batches do not establish exhaustive coverage. Dated individual follow-ups do not block independent discovery.

Hosted catalogues have separate ledgers and readbacks under `hosted/`, with ownership limited to the exact merchant catalogue. `country-verification.json` aggregates the root and hosted checks; `verification.json` remains the root-ledger readback. The country index includes all CH-managed sources.

## Current checkpoint

ch-2026-11-03-091, 2026-11-03T19:51:58.217996Z. Country totals: 155 ADDED, 141 DEFERRED, 38 EXCLUDED. All root and hosted verification checks passed; see `country-verification.json`. Paid-data spend USD 0. No global blocker; all cantons remain open.

Added Galerie des Annonciades with a specific current Polper painting, declared Swiss contracting seller, VirtueMart, advertised partial delivery coverage and explicit arranged gallery collection. Read back the retained source/location IDs and delivery profile after confirming the initial HTTP400 profile rejection created no resource and correcting required nullable zone applicability. Excluded Klaus Zurfluh for restoration/new custom work only; deferred Maurus Nann and Antikmarkt Opfertshofen. No uncertain writes remain; ingestion stays empty.

## Continuation

Resume queued leads and dated follow-ups in `frontier.jsonl`. Root and every hosted ledger require explicit audit, index and verification paths after each batch; every fifth batch receives full readbacks. Hosted duplicate checks compare the exact merchant namespace plus Party identity, never claim the provider host. Country-local orchestration is under `/tmp/aura-dealer-discovery/CH/`; raw evidence and credentials remain external.
