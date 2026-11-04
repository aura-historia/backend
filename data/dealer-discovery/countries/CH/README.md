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

ch-2026-11-04-160, 2026-11-04T11:15:48.359701Z. Country totals: 230 ADDED, 256 DEFERRED, 57 EXCLUDED. Paid-data spend USD 0. Country remains open.

Directory/ownership research batch: Swissportail80entries sampled, no directory statements imported. Servan,Votano and2ndLife held in frontier for current primary seller/stock; separateKlangcatalogue country/operator relationship unresolved, existingCHSchatzsuche identity not duplicated. Antik4you/Petrollampen/Puppenland already DEowned, fetches skipped. No new dealer/API mutations. Full resource160 initiated at required fifth-batch interval; explicit audit/index/verification preserve external ingestion and honest dates. Country remains open; free research continues.

Strict root and hosted verification failed: 70 sources now have externally configured ingestion. All sources were created with empty ingestion; this workflow preserves external changes. See `country-verification.json` for current observations and actual last successful verification. New resource/default creation readbacks passed. This limits country-wide verification, not independent research. Latest full resource integrity readback: `resource-verification.json` (its own dates/counts apply).

## Continuation

Continue queued leads and dated follow-ups in `frontier.jsonl`; check all country ownership before mutation. Run explicit country-ledger audits, indexes and verification after each batch. Preserve external ingestion changes and report strict failures honestly; never enable ingestion during onboarding. Raw evidence and temporary orchestration remain under `/tmp/aura-dealer-discovery/CH/`.
