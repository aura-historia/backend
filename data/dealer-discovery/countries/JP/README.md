# Japan dealer discovery

Owner: Japan (JP), continuous discovery started 2026-11-03. JP is Japan’s ISO 3166-1 alpha-2 code. Country assignment uses verified business home country; existing ledger ownership takes precedence, including foreign dealers retained by Germany.

[Ledger](dealers.jsonl) journals reviewed evidence and API mutations; [frontier](frontier.jsonl) holds leads, ownership queues and dated follow-ups; [searches](searches.jsonl) records free search coverage; [index](index.tsv) is generated from this explicit ledger; [verification](verification.json) records explicit country live readback. Raw pages, snapshots and asset previews stay under `/tmp/aura-dealer-discovery/JP/`. Credentials remain external in the existing owner-only file.

Only current first-party facts assessed strictly above 95% confidence are imported. Only independent own-stock dealers with direct art and antiques relevance qualify. Modern decor, general resale and recent secondhand fashion alone do not qualify. Marketplaces and auction operators are excluded. Discovery creates sources with empty ingestion and never enables it; later external changes are reported separately. Registered addresses, business premises, stock, dispatch and collection are distinct; unknown shipping stays unknown.

## Current checkpoint

jp-2026-11-04-126, 2026-11-04T10:52:12.570236Z. Country totals: 253 ADDED, 203 DEFERRED, 44 EXCLUDED. Audit/index passed; current verification failed for 66 external ingestion changes. The last passing full readback is jp-2026-11-03-070 at 2026-11-03T20:28:05.584720Z; `verification.json` retains that dated result. See [verification drift](verification-drift.json). USD 0 spent; countrywide research remains open.

Batch 126 added Yoseido Gallery’s current Wix Stores original-art catalogue under verified 有限会社 阿部養清堂. Inspected an available edition-one woodblock and a separate drawing; current rebuilt Ginza premises remain distinct from the expired temporary address. Domestic and unspecified international delivery retain null rates. Separate BASE catalogue queued under the same owner; migrated Shinseido and Soumeido leads resolved to existing Japan owners. One addition, no exclusions or deferrals. Country totals 253 added, 203 deferred, 44 excluded. Audit/index passed; explicit incremental verification failed on the same 66 external ingestion changes confirmed by fresh readbacks. Dated PASS070 retained. USD 0; no pending mutations or global blocker; research continues.

## Continuation

Continue queued frontier and regional/category searches in bounded batches until stopped. Check due reviews and reconcile incomplete writes first. Pass explicit Japan ledger/output paths to all helpers. Audit, regenerate index, verify and update these notes after every batch; full verification initially and at least every fifth batch.
