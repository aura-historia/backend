# Owner review: DACH run 2026-11-25, owner decisions applied 2026-11-26

## Decisions the owner gave on 2026-11-26 (now policy, see `/tmp` BRIEF and the country READMEs)
- Out-of-policy live dealers may be removed at the orchestrator's judgement (contemporary art, post-war art mixes, new goods).
- **No vintage/new mixes.** New goods beyond a trivial share (> ~5% of items, or a dedicated new-goods section) -> excluded `NEW_GOODS_MIX`
  (reissues, replicas/Nachbauten/Sammleranfertigungen, made-to-order new pieces, new gifts/decor, new watches, newly minted coins and bullion).
- Vintage watches are fine; model railways are not. Nazi-era literature/militaria sellers are fine. Commission/consignment selling is fine
  (shops that sell third-party items on commission). Auction-format sellers stay excluded.
- Modern/post-war art: only dealers whose stock is predominantly pre-1945 with <= 5% contemporary; the old `MODERN_ART_OWNER_DECISION` deferral was retired.

## What was done
- [removed-2026-11-26.tsv](removed-2026-11-26.tsv): 115 dealers deleted from the backend (source deleted and read back as 404), each with an `EXCLUDED` ledger row
  that keeps the removed ids under `backend.removed`. DE 70, AT 20, CH 25. Reasons: NEW_GOODS_MIX 63, CONTEMPORARY_ART 30, POST_WAR_MODERN_ART 10,
  OUTSIDE_SPECIALIST_ANTIQUES_SCOPE 11, NEW_GOODS_RETAIL 1. The operator Parties could not be deleted (the API refuses while a Party keeps locations and offers
  no location delete); the follow-up below cleaned those up in the database.
- [removal-blocked-has-listings.tsv](removal-blocked-has-listings.tsv): 85 dealers that should be removed but could not be deleted through the API first: it only deletes a listing
  source that has no ProductListings, and these already had ingested listings (DE 61, AT 13, CH 11). **All 85 are now removed** (owner authorised direct database deletion and
  the operator's OpenSearch access, 2026-11-26). Order: ingestion off, then per source (`tools/purge.py delete`): row archive, `DELETE` of all 115,434 listings (any lifecycle; FK
  cascades remove their owned rows) and 117,756 raw streams in small chunks, then the source's search documents were replaced with the application's own withdrawal tombstones
  (`{productListingId, projectionDeleted: true}` at the document's current external version, written through the operator's loopback admin endpoint documented in
  `docs/opensearch-stage.md`; tombstones are kept on purpose, see the durable-worker runbook). Then `remove.py --all --execute` deleted the 85 sources through the API and the orphan
  Party cleanup ran again. Verification: 0 listings/streams left in the database, 0 live search documents for the 85 sources, every source in the index exists in the database,
  the API answers 404 for a deleted listing, worker `job_queue_failures` unchanged at 14. A pilot on the smallest source ran first.
- [borderline-added.tsv](borderline-added.tsv): the earlier judgement-call list, now with the current status per dealer (10 were removed as mixes/scope).
- [kept-borderline-2026-11-26.tsv](kept-borderline-2026-11-26.tsv): dealers the reviews flagged but that were kept (trivial share, not provable, or scope calls for the owner).
- [out-of-policy-live.tsv](out-of-policy-live.tsv): the original 66-dealer list; all were removed or verified (the blocked ones after the direct deletion above).

- **Orphan cleanup in the database (owner authorisation: orphaned parties, locations, product listings of the removed sources only).** Of the 113 operator Parties of the
  115 removed sources: 56 Parties deleted with all their locations, location-change rows and geo bindings; 4 kept because they also operate live dealers
  (antik4you.ch, szaal.com, 1.august.ch, steutzger.*); 53 kept because shipping profiles are attributed to them (`shipping_profile_history` is immutable by design and the
  profiles reference the Party). Of 139 locations 110 were deleted; 29 stay because a shipping profile references them. The row copies were shredded afterwards (see Housekeeping).
  Ledger rows `...-party-cleanup` record the Parties that are gone. Second pass after the 85 listing deletions: 27 more Parties and 91 locations deleted, 3 Parties kept (they still operate live dealers: szaal.at, schweizergeld.ch, steutzger.info), 110 kept
  because shipping profiles are attributed to them (immutable history). Tool: `tools/purge.py` (phases status, ingestion-off, withdraw, purge, delete, orphans; guarded by the removal allowlist).
- **Marketplace-only dealers: removed (owner decision).** 35 deferred `MARKETPLACE_ONLY_STOCK_OWNER_DECISION` dealers are now `EXCLUDED` (`MARKETPLACE_ONLY_STOCK`, DE 28 / AT 4 / CH 3);
  27 further DE deferrals that record the identical fact under other reason codes got the same status under the run name `...-owner-marketplace-only-same-class` (ledger rows only; reversible by a new row).

## Second round of owner decisions (2026-11-26, later)
- **Parties held by shipping history: removed, including their shipping.** 114 operator Parties of removed dealers were deleted with 117 shipping profiles, 120 history rows, 120 evidence
  rows, 117 creation receipts and 35 location references (`tools/purge.py orphans --with-shipping`). This is the explicit maintenance procedure `docs/shipping.md` asks for: one transaction per
  Party, the three immutability triggers disabled only inside it and re-enabled before commit (verified enabled afterwards), a guard that nothing outside the Party references its profiles. The
  content-free `shipping_changes` / `shipping_validity_changes` signal rows (identity and revision only, no terms or addresses, no foreign keys) were left in place. 3 Parties stay because they
  still operate live dealers (szaal.at, schweizergeld.ch, steutzger.info). The row copies were shredded afterwards (see Housekeeping).
- **Auctions: rule B (owner, 2026-11-26).** Only businesses that mainly ARE auctions are out (auction houses, auction platforms, auction brokerage, auction-format-only sellers, a shop that is the
  retail corner of an auction house). Dealers with an own priced webshop and auctions as a side activity stay eligible. This replaced the first reading of the day, which excluded 26 deferrals
  wholesale, and the older blanket rule ("any dealer that also runs auctions"; the repo `SKILL.md` still words it that way and was not edited). Re-review of 260 dealers (234 earlier auction exclusions + the
  26): 105 plainly auction businesses stayed out; 155 were put back to DEFERRED (`AUCTION_RULE_B_RECHECK`) and re-read live by seven agents plus two lead agents: **23 added** (all with empty
  ingestion; each has an "AUCTION SIDE ACTIVITY" note in [rule-b-added.tsv](rule-b-added.tsv)), 38 excluded as auction businesses (evidence in the ledger rows), the rest excluded for other rules
  (contemporary art, new goods, marketplace-only, scope, no inspectable stock) or deferred (`AUCTION_SHARE_AMBIGUOUS`, review 2027-02-24). The 2 live dealers with an unresolved auction role
  (salledesventes.ch, galerie-antik-erzgebirge.de) stay.
- **Scope calls: only vintage or better antique stock.** katharina-vintage.com, nikonclassics-michalke.de, porzellan-porcelain.de and swisswatches-andmore.com were removed from the backend
  (nikonclassics 375 and swisswatches 1,382 listings deleted, search documents tombstoned); schmucklicher.ch and till-weber.de are `EXCLUDED` (`OUTSIDE_SPECIALIST_ANTIQUES_SCOPE`).
- **Kirchner/Potthof single-artist shops:** stay excluded.

## Owner confirmations (2026-11-26, last)
- The repo skill (`.agents/skills/discover-antique-dealers/SKILL.md`) and `data/dealer-discovery/README.md` now state rule B (edited; not committed).
- All closest calls of rule B are valid as decided: the kept dealers with a sister auction company or an auctioneer proprietor (salzburger-muenzhandlung.at, pater.de, richter-historica.de,
  militaria-berlin.de, poster-auctioneer.com) stay; peus-muenzen.de and muenzen-moeller.de stay excluded as auction-led.
- Posters from 1970-1999 do not count as vintage (poster-gallery.com and awarded-poster.com stay excluded).

## Housekeeping (2026-11-26, on the owner's "do the housekeeping things")
- **Geo resolutions:** the backend's own cleanup only blanks the evidence of a resolution that lost all its bindings and never deletes the row, so the rows of the removed dealers' locations
  stayed. The 136 business resolutions whose bindings were archived when the locations were removed were deleted with their 400 change rows and 313 wake-up rows (`purge.py geo`).
  118 unbound rows remain on purpose: 108 business resolutions from 1-25 November that no removal of mine produced (earlier address corrections or removals by others) and 10 erased
  private-address tombstones of users. They are inert; none of them is attributable to a dealer removed here.
- **Crawler state:** the crawler's own database (`crawler_server`) kept 184 rows of the removed sources, all `crawl_enabled=false` and out of scope. They were deleted source by source
  (`purge_crawler.py`; the schema's cascade removed 333,102 URLs, 231,712 frontier rows, 87 product schemas, 1 removed-page schema, 284 reviews, 130 label events and 249 qualification
  events; the append-only triggers allow exactly that cascade, none was disabled). The crawler stayed healthy; no row of any other source was touched. Disk space comes back only after
  autovacuum/`VACUUM FULL`, which was not run.
- **Purge archive:** the copies of deleted rows, including the shipping content, were overwritten and removed with `shred -u` (ext4, one random pass; on RAID/SSD an overwrite is not a
  hardware guarantee). Kept: the removal journal (ids and counts only), the allowlist, and `reports/crawler-removed-sources.jsonl` (name, slug, host and label of the 184 crawler rows).

## Ceiling check and coverage estimate (2026-11-26, on the owner's questions "really at the ceiling?" and "how good is the coverage?")
Earlier saturation tests only used web search. This check sampled sources no earlier run had used and counted how many of the *qualifying* dealers found there were already in the ledgers
(capture-recapture; a dealer qualifies when it ends as ADDED under the current policy):

| Frame (date 2026-11-26) | DE | AT | CH |
| --- | --- | --- | --- |
| OpenStreetMap antique shops with website, 761 sites | 95% (126 of 133) | 94% (15 of 16) | 100% (11 of 11) |
| Das Örtliche, 1,575 shop websites (DE only) | 97% (140 of 144) | - | - |
| search.ch directory, 88 websites (CH only) | - | - | 100% (14 of 14) |
| 104 fresh product-led web searches, 372 dealer-like domains | **94%** (64 of 68, CI 86-98) | **96%** (25 of 26, CI 81-99) | **92%** (44 of 48, CI 80-97) |

- **Reading:** about 94% of the qualifying dealers that free sources show were already added before this check (all three countries together 133 of 142). The ledgers now hold ADDED DE 896, AT 141,
  CH 232. Frame details: the directories contain only 26% (DE), 12% (AT) and 5% (CH) of our added dealers, so they measure a slice; the search sample reaches the web-only majority.
- **Estimated coverage of all eligible web shops:** about **85-93% Germany, 85-93% Austria, 85-93% Switzerland** after allowing for what the method cannot see (dealers that neither directories nor web search
  surface, and the positive dependence between our earlier searches and this one). Dealers still to find: roughly 50-150 in DE, 5-30 in AT, 20-60 in CH, plus a few dozen deferred ones.
  The Austria figure rests on 26 dealers, Switzerland 48, so their intervals are wide.
- **Cost of the remaining dealers:** 24 dealers were added by this check (DE 17, AT 2, CH 5) from about 460 leads; 12 of them are flagged borderline (list in `ceiling-check-added.tsv`). The
  remaining findable dealers are increasingly marginal: curated shops of general Brockenstuben, retiring owners, single-page shops, repair workshops with a few antiques, partly 20th-century stock.
- Frames harvested: OSM Overpass (`shop=antiques`, collector, coins, antiquariat, art/antique name matches), Das Örtliche (about 120 cities x Antiquitäten/Antiquariate/Kunsthandlungen), search.ch
  (40 cities x 3 categories). Gelbe Seiten and 11880 list no websites; local.ch answers 403; one Herold category URL I tried returned nothing to the fetcher (earlier runs did use Herold pages).

### Decisions taken on the owner's delegation (2026-11-26)
- **Rule for borderline cases:** the stock must be predominantly (more than half) antique or vintage, meaning made before about 1970 (now in the skill). A dealer with a few priced items qualifies; size alone is not a criterion.
- **Removed (5 live dealers, Parties erased with their shipping, orphan geo resolutions deleted, archive shredded):** antiquariat-held.de, ulrich-doege.de, antiquariat-abaton.de (mostly post-1945 used books),
  rudisladen.de (1960s-80s nostalgia, mostly after 1970), villaterra-shop.de (prop rental and salvage with gift ideas and remainders).
- **Kept:** antikpmw.biz, antikburmeister.de (dolls, Steiff, clocks, porcelain: predominantly antique), shop.brockiwolf.ch, uhrmacheratelier.ch (review 2027-05-26), allerdingsantiquariat.ch, fabulafero.ch, uhren-brunner.ch,
  gablitzer-antiquariat.at, galerie-kunststuecke.de, traudeskinder.com.
- **Closest exclusions stay excluded:** kunsthandel-henneken.de (about 14% post-war/living artists, owner limit is 5%), altertum.ch (dedicated "Neuanfertigungen" section), fineart-selection.de (post-war share, no prices).
- **akpool.de:** `EXCLUDED` `CATALOGUE_TOO_LARGE_PARTNER_CANDIDATE`. New general rule: no listing source with 300,000 or more product listings (about 1.8 million cards here); such catalogues become partners later.
  No live source is affected: the largest has 48,751 listings.
- **DACH stops here.** Measured coverage 94% of the qualifying dealers that free sources show; the dealers still to find are few, marginal and expensive. Ledger ADDED DE 891, AT 141, CH 232 (live and verified).
  Discovery moves to the UK, France and Italy.

## Housekeeping leftovers
- 108 unbound business geo resolutions from 1-25 November remain (no removal of mine produced them) and 10 erased user-address tombstones; all inert.
