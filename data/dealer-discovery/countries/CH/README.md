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

ch-2026-11-03-108, 2026-11-03T21:13:59.617491Z. Country totals: 177 ADDED, 159 DEFERRED, 43 EXCLUDED. Paid-data spend USD 0. Country remains open.

Added Swiss Rarity Clocks under active Swiss Uhrenwerkstatt and Curta-Service M. Schneider, specifically from available circa1820 Sumiswald antique pendule. Inspected original logo, Jimdo and current145-label shipping table:141 canonical ISO areas plus4 unresolved historical/ambiguous labels, with foreign bulky-clock restrictions and independent workshop collection preserved. Reconciled rejected profile before retry: empty profile/default readback, retained all source/Party/location IDs and creation key, partitioned same-rate88-area zone into64/24 with no semantic change. All created resources/profile/defaults read back; ingestion empty. Existing external ingestion differences preserved. USD0, country open.

Strict root and hosted verification failed: 70 sources now have externally configured ingestion. All sources were created with empty ingestion; this workflow preserves external changes. See `country-verification.json` for current observations and actual last successful verification. New resource/default creation readbacks passed. This limits country-wide verification, not independent research. Latest full resource integrity readback: `resource-verification.json` (its own dates/counts apply).

## Continuation

Continue queued leads and dated follow-ups in `frontier.jsonl`; check all country ownership before mutation. Run explicit country-ledger audits, indexes and verification after each batch. Preserve external ingestion changes and report strict failures honestly; never enable ingestion during onboarding. Raw evidence and temporary orchestration remain under `/tmp/aura-dealer-discovery/CH/`.
