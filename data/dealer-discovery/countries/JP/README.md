# Japan dealer discovery

Owner: Japan (JP), continuous discovery started 2026-11-03. JP is Japan’s ISO 3166-1 alpha-2 code. Country assignment uses verified business home country; existing ledger ownership takes precedence, including foreign dealers retained by Germany.

[Ledger](dealers.jsonl) journals reviewed evidence and API mutations; [frontier](frontier.jsonl) holds leads, ownership queues and dated follow-ups; [searches](searches.jsonl) records free search coverage; [index](index.tsv) is generated from this explicit ledger; [verification](verification.json) records explicit country live readback. Raw pages, snapshots and asset previews stay under `/tmp/aura-dealer-discovery/JP/`. Credentials remain external in the existing owner-only file.

Only current first-party facts assessed strictly above 95% confidence are imported. Only independent own-stock dealers with direct art and antiques relevance qualify. Modern decor, general resale and recent secondhand fashion alone do not qualify. Marketplaces and auction operators are excluded. Discovery creates sources with empty ingestion and never enables it; later external changes are reported separately. Registered addresses, business premises, stock, dispatch and collection are distinct; unknown shipping stays unknown.

## Current checkpoint

jp-2026-11-03-078, 2026-11-03T21:15:58.909135Z. Country totals: 198 ADDED, 137 DEFERRED, 23 EXCLUDED. Audit/index passed; current verification failed for 66 external ingestion changes. The last passing full readback is jp-2026-11-03-070 at 2026-11-03T20:28:05.584720Z; `verification.json` retains that dated result. See [verification drift](verification-drift.json). USD 0 spent; countrywide research remains open.

Batch78 added KeiseiArt (敬誠株式会社) through a current priced original OldNoritake vase and recent2026trade evidence; new resources/defaults read back with empty ingestion. Abiko gallery distinguished from unlocated warehouse and shipping fee exceptions preserved without universal free rate. Garage24 deferred for missing current own retail stock/legal identity; KahmArt excluded for explicit own auctions. Existing Genroku, MoriokaBijutsu and Morimiya ownership retained without duplicate sources. Audit/index passed; country verification still fails for66 external WEB_CRAWL sources, freshly confirmed read-only and preserved. Last passing full result remains batch70. No pending writes/global research blocker, USD0; continuing directory expansion.

## Continuation

Continue queued frontier and regional/category searches in bounded batches until stopped. Check due reviews and reconcile incomplete writes first. Pass explicit Japan ledger/output paths to all helpers. Audit, regenerate index, verify and update these notes after every batch; full verification initially and at least every fifth batch.
