# Japan dealer discovery

Owner: Japan (JP), continuous discovery started 2026-11-03. JP is Japan’s ISO 3166-1 alpha-2 code. Country assignment uses verified business home country; existing ledger ownership takes precedence, including foreign dealers retained by Germany.

[Ledger](dealers.jsonl) journals reviewed evidence and API mutations; [frontier](frontier.jsonl) holds leads, ownership queues and dated follow-ups; [searches](searches.jsonl) records free search coverage; [index](index.tsv) is generated from this explicit ledger; [verification](verification.json) records explicit country live readback. Raw pages, snapshots and asset previews stay under `/tmp/aura-dealer-discovery/JP/`. Credentials remain external in the existing owner-only file.

Only current first-party facts assessed strictly above 95% confidence are imported. Only independent own-stock dealers with direct art and antiques relevance qualify. Modern decor, general resale and recent secondhand fashion alone do not qualify. Marketplaces and auction operators are excluded. Ingestion stays empty. Registered addresses, business premises, stock, dispatch and collection are distinct; unknown shipping stays unknown.

## Current checkpoint

jp-2026-11-03-063, 2026-11-03T19:51:43.666239Z. Country ledger totals: 171 ADDED, 113 DEFERRED, 17 EXCLUDED. Audit/index and explicit country verification passed; see `verification.json`. Paid-data spend USD 0. No global blocker; countrywide coverage remains open.

Added Taiko, Yamada Yukodo and Kaino using verified original painting/scroll stock and Japan operators. Preserved Taiko regional freight exceptions, Yamada’s unknown platform/shipping, and Kaino’s department-specific delivery restrictions. Legal/HQ addresses were not promoted to visitor or stock premises. Deferred Yamamoto’s hosted catalogue for unavailable individual stock and inconsistent legal/contact declarations. All 171 sources passed audit/index and incremental verification with empty ingestion. No pending writes or global blocker; USD 0 spent. Continuing batch 64; next full readback due65.

## Continuation

Continue queued frontier and regional/category searches in bounded batches until stopped. Check due reviews and reconcile incomplete writes first. Pass explicit Japan ledger/output paths to all helpers. Audit, regenerate index, verify and update these notes after every batch; full verification initially and at least every fifth batch.
