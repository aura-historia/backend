# Japan dealer discovery

Owner: Japan (JP), continuous discovery started 2026-11-03. JP is Japan’s ISO 3166-1 alpha-2 code. Country assignment uses verified business home country; existing ledger ownership takes precedence, including foreign dealers retained by Germany.

[Ledger](dealers.jsonl) journals reviewed evidence and API mutations; [frontier](frontier.jsonl) holds leads, ownership queues and dated follow-ups; [searches](searches.jsonl) records free search coverage; [index](index.tsv) is generated from this explicit ledger; [verification](verification.json) records explicit country live readback. Raw pages, snapshots and asset previews stay under `/tmp/aura-dealer-discovery/JP/`. Credentials remain external in the existing owner-only file.

Only current first-party facts assessed strictly above 95% confidence are imported. Only independent own-stock dealers with direct art and antiques relevance qualify. Modern decor, general resale and recent secondhand fashion alone do not qualify. Marketplaces and auction operators are excluded. Discovery creates sources with empty ingestion and never enables it; later external changes are reported separately. Registered addresses, business premises, stock, dispatch and collection are distinct; unknown shipping stays unknown.

## Current checkpoint

jp-2026-11-04-157, 2026-11-04T14:47:10.236326Z. Country totals across 4 explicit ledgers: 272 ADDED, 246 DEFERRED, 51 EXCLUDED. All ledger audits and generated indexes passed. Current main verification failed for 66 external ingestion changes; hosted verification passed for its own current scope. The last passing main-ledger full readback is jp-2026-11-03-070 at 2026-11-03T20:28:05.584720Z; `verification.json` retains that dated result and predates hosted additions. No current passing country aggregate is claimed. See [current verification drift and ledger reports](verification-drift.json). USD0; country coverage remains open.

Batch 157 deferred Eizawa and Maruo because current primary pages verify active Japanese buying businesses but no current inspectable own-stock sale offers. Buying examples, artist articles and inbound free postage were not promoted to sale stock or delivery. Eizawa linked buying domains remain research under the same operator, without new sources or aliases. Ide was excluded on explicit own trade-auction hosting, with proprietor identity from first-party terms. All-country recursive duplicate checks cleared; no API mutations. All four ledgers audited, indexed and verified; the same external ingestion drift is preserved. USD0, continue.

## Continuation

Continue queued frontier and regional/category searches in bounded batches until stopped. Check due reviews and reconcile incomplete writes first. Pass explicit Japan ledger/output paths to all helpers. Include every nested hosted ledger in all subsequent audits, indexes and verification; generate the main index before aggregating it with the nested indexes. Audit, regenerate index, verify and update these notes after every batch; full verification initially and at least every fifth batch.

## Hosted merchant ownership

[hanzomon](hosted/hanzomon/README.md), [imado](hosted/imado/README.md), [maruhira](hosted/maruhira/README.md) each owns only its exact merchant catalogue. The platform and other merchants are outside that ownership. Nested indexes and verification are separate; country totals/index aggregate all explicit ledgers. `verification-drift.json` lists current per-ledger verification attempts while the historic main passing report remains unchanged. No platform-root alias is claimed.
