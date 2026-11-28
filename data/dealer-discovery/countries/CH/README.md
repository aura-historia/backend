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

ch-2026-11-04-215, 2026-11-04T17:06:54.677611Z. Country totals: 272 ADDED, 325 DEFERRED, 66 EXCLUDED. Paid-data spend USD 0. Country remains open.

Added MÉROUÉE Sandra Wirth Nikitine, specific current early19thcentury articulated silver spice-fish pair CHF1095/cart and actual physical stock photo inspected. Current native full business footer confirms RutiCH and named business owner, correspondence street without house number only; no premises/stock/pickup invented. Actual branded photographic wordmark inspected and CM4all Shop independently identified by concrete native widget/runtime; weak WordPress hint rejected. Partial CH plus unspecified foreign delivery on prior enquiry, fees/times depend mutual agreement, unknown prices preserved. Empty ingestion and every created resource/default read back. Two exclusions: GMFK current reproduction publisher store with originals retained/priced by individual artists, no allocated own original unit; OldFashion current bare/www domain-sale parking without former-business closure or current operator country claim. Legacy unavailable clock/postcard/furniture domains queued with home unverified. FULL215 resource integrity PASSED all272 sources, zero failures,202empty ingestion and70externally configured preserved. Explicit audits/index/strict country verification repeated, still limited by ingestion drift. USD0, no global blocker, all cantons open.

Strict root and hosted verification failed: 70 sources now have externally configured ingestion. All sources were created with empty ingestion; this workflow preserves external changes. See `country-verification.json` for current observations and actual last successful verification. New resource/default creation readbacks passed. This limits country-wide verification, not independent research. Latest full resource integrity readback: `resource-verification.json` (its own dates/counts apply).

## Continuation

Continue queued leads and dated follow-ups in `frontier.jsonl`; check all country ownership before mutation. Run explicit country-ledger audits, indexes and verification after each batch. Preserve external ingestion changes and report strict failures honestly; never enable ingestion during onboarding. Raw evidence and temporary orchestration remain under `/tmp/aura-dealer-discovery/CH/`.

## 2026-11-25 DACH parallel run (CH)

Runs `2026-11-25-ch-par-*`: **+48 dealers ADDED (259 now)**, 289 EXCLUDED (89 `CONTEMPORARY_ART`) and 169 DEFERRED decisions, read back live with empty ingestion. Paid-data spend: $0.

- Plain re-checks and association lists were saturated; blocked-domain niche web searches (vintage design Zürich/Winterthur/Basel, postcards, coins, antiquarian books, stoves, militaria) found most new dealers, but conversion fell to about 5% in the last round.
- Swiss shops publish German, French or Italian imprints and price in CHF; shipping thresholds are recorded in CHF. The official UID register (`uid-wse.admin.ch`, about 10 requests per hour per IP) resolves sole-proprietor and legal operators when a site names no legal form; it supports, never replaces, the site's own statement.
- Open owner decisions: see [the owner review](../../owner-review-2026-11-25/README.md). Deferred hosts are due again 2027-01-09, 2027-02-25 and 2027-05-25.

## 2026-11-26 owner decisions and treg search follow-up (CH)
- Owner rules applied (no vintage/new mixes, no contemporary/post-war art mixes; see the owner-review README).
- 25 live dealers were removed from the backend; 11 more had ingested ProductListings, so the API could not delete them; the owner authorised direct database deletion and they were removed afterwards (`removal-blocked-has-listings.tsv`): 36 CH dealers removed in total.
- 1 dealer was added from treg.to searches (lodeco.ch); the 24 deferred `MODERN_ART_OWNER_DECISION` galleries were re-checked under the new rule and all excluded.
- Swiss search yield is near saturation (3 rounds: 1 addition from about 24 leads).

### 2026-11-26 follow-up: marketplace-only dealers removed, orphan cleanup
- Owner ruling: dealers whose stock exists only on third-party marketplaces are removed. 3 deferrals (`MARKETPLACE_ONLY_STOCK_OWNER_DECISION`) are now `EXCLUDED` (`MARKETPLACE_ONLY_STOCK`).
- Orphan Parties/locations of the dealers removed on 2026-11-26 were cleaned up where nothing references them (`../../owner-review-2026-11-25/README.md`). The 11 CH dealers that held ingested listings were removed later the same day (listings and raw streams deleted in the database, search documents replaced by tombstones, sources and orphan Parties deleted). Live ledger count after the removal: CH 224 `ADDED`.
- Second round of owner decisions (2026-11-26, later): 4 deferred dealers with an unresolved auction role are `EXCLUDED` (`AUCTION_ROLE_OWNER_DECISION`); swisswatches-andmore.com removed from the backend and schmucklicher.ch `EXCLUDED` (scope); the Kirchner/Potthof shops stay excluded. Operator Parties held by shipping history were erased with their shipping (see `../../owner-review-2026-11-25/README.md`). Live ledger count: CH 223 `ADDED`.
- Rule B on auctions (owner, 2026-11-26): 30 CH dealers were re-read live; 3 added (swissposterart.com, antik-unique.ch, cahn.ch) plus poster-auctioneer.com (second storefront of the poster operator), see `../../owner-review-2026-11-25/rule-b-added.tsv`. Live ledger count: CH 227 `ADDED`.
