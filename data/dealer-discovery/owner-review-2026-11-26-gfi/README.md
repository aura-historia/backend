# UK / France / Italy discovery campaign (2026-11-26)

The owner delegated the continuation: DACH stopped (measured coverage 94%), discovery restarted for the United Kingdom, France and Italy with the current policy. The earlier country runs
(2026-11-01..03) used an older policy, so the campaign has three parts: audit the live dealers against the current policy, re-qualify the exclusions, discover new dealers.
Ledgers: `countries/GB`, `countries/FR`, `countries/IT` (hosted-merchant ledgers untouched). Orchestrator workspace (not in the repo): `/tmp/aura-dealer-discovery/GFI-par/`.

## Policies applied (all in the skill and in the orchestration brief)
- No contemporary art; no vintage/new mixes (dedicated new-goods section or more than about 5% new items); auctions only out when the business mainly is an auction business; marketplace-only out;
  stock predominantly (more than half) antique or vintage (before about 1970); no catalogue of 300,000 or more listings (partner candidate, `CATALOGUE_TOO_LARGE_PARTNER_CANDIDATE`).
- **Policy D (decided by the orchestrator on the owner's delegation):** an itemised catalogue of identifiable antique items qualifies even when every price is "on request", matching the platform's own
  crawler contract. Sites without itemised stock (photo galleries without item data, business cards, exhibition-only) stay out. Earlier price-related exclusions were re-reviewed.

## Audit of the existing live dealers
Triage flags (auction, contemporary, new-goods wording) selected 367 of the 1,078 live dealers; 12 read-only audit agents checked each against the current policy: **227 keep, 107 out of policy, 33 unsure**
(UK 93 audited: 23 out; FR 195: 63 out, 26 of them not antique majority; IT 79: 20 out; plus troc29.fr, closing at year end). The 81 clear cases (confidence 0.8 or higher; UK 19, FR 50, IT 12) were removed: 71 sources without listings through the guarded tool, 10 with
35,683 listings (crawl switched off, listings deleted, search documents tombstoned), 79 orphan Parties erased with their shipping profiles (63), 102 locations and 102 geo resolutions deleted, row archive shredded.
Not removed: 25 weaker OUT verdicts and 33 unsure ones stay live (listed in the orchestrator workspace `reports/audit-decision.json`). troc29.fr closes on 31 December 2026 and stays until then.

## Coverage of the earlier ledgers (capture-recapture on fresh frames)
Share of the qualifying dealers found in each frame that were already in the ledgers before this campaign:

| Frame | GB | FR | IT |
| --- | --- | --- | --- |
| 96 fresh product-led web searches | 44% (26 of 59) | 62% (24 of 39) | 75% (30 of 40) |
| Association frames (LAPADA/PBFA; French SLAM/Carré Rive Gauche; Italian AAI/ALAI/ILAB) | 54% (60 of 111) | - (3 dealers) | 91% (75 of 82) |

So the UK was the thinnest ledger, then France, then Italy. The new dealers listed below close part of that gap.

## Re-review under Policy D (2026-11-27)
The 67 exclusions that only lacked published prices (UK 24, FR 22, IT 21) went back through the re-check flow: **41 qualified** (UK 12, FR 13, IT 16). Several "no prices" findings were simply wrong (prices are on view at
Antique Karen, Clive Payne, Denton, Métais, Ythurbide). Two French galleries (galeriedelalande.fr, ratton-ladriere.com) stay deferred because they sell their own new books in a dedicated section.

## State after the re-review (live and verified)
| | UK | France | Italy |
| --- | --- | --- | --- |
| Added dealers (live) | 365 | 441 | 354 |
| Before the campaign | 286 | 460 | 332 |
| Removed as out of policy | 19 | 50 | 12 |
| New dealers from leads (wave 1) | 86 | 18 | 18 |
| Re-qualified under Policy D | 12 | 13 | 16 |

Running next: qualification of 372 further leads from OpenStreetMap (UK 208, FR 124, IT 40). Weaker audit verdicts (25 out, 33 unsure) stay live for now.

## OpenStreetMap leads (2026-11-27)
372 triaged OSM leads (UK 208, FR 124, IT 40) went to 11 qualifier agents: **70 dealers added** (UK 45, FR 15, IT 10), about 19% of the leads (UK 22%, FR 12%, IT 25%), much better than in DACH (4%) because the UK and French ledgers
were thin. Typical misses: multi-dealer antique centres, shops that only show an Instagram feed, brochure sites, new-goods retail.

| | UK | France | Italy |
| --- | --- | --- | --- |
| Added dealers (live, verified) | 410 | 456 | 364 |

Quality flags to keep in mind (all recorded in the ledgers): many UK and French dealers publish no operator or imprint (operator = trading name, confirmed by Companies House/SIRENE when possible); a number of
added dealers have price-on-request catalogues (Policy D) or only locality-level locations.

## Town-led searches and coverage after three waves (2026-11-27)
96 fresh town-led queries (UK 32, FR 32, IT 32; "antiquaire <ville> vente en ligne ... mentions légales" and equivalents) found 387 dealer-like websites; 166 live NEW leads went to 5 qualifier agents: **20 dealers added** (UK 8, FR 7, IT 5).

Coverage of the qualifying dealers in that independent sample, measured after the earlier waves (association/OSM/search) but before these 20 were added:

| | UK | France | Italy |
| --- | --- | --- | --- |
| Before the campaign (96 product-led searches, association frames) | 44-54% | 62% | 75-91% |
| After waves 1-2 (town-led sample) | 73% (22 of 30, CI 56-86%) | 79% (27 of 34, CI 63-90%) | 91% (49 of 54, CI 80-96%) |

Added dealers now (live, verified): **UK 418, France 463, Italy 369**. Italy is close to the level reached in DACH; UK and France still show gaps, so further rounds there are worthwhile, with diminishing yield
(this round: 8 of 84 UK leads, 7 of 75 FR leads; many leads were buying services, US sellers, multi-dealer centres or Instagram-only sites).

## Niche-by-region searches (round four, 2026-11-27)
64 queries "<niche> <region> dealer online shop ..." for the UK and France (clocks Kent, pewter, arms, maps Scotland, faïence Bretagne, horlogerie Jura ...) found 172 NEW leads; 140 live ones went to 4 qualifier agents:
**40 dealers added** (UK 24, FR 16). The UK yield was the best of any search round (24 of 77 leads).

Coverage of the qualifying dealers in this sample, measured before these 40 were added: **UK 48% (21 of 44, CI 34-62%), France 58% (22 of 38, CI 42-72%)**. Specialist niches were thinly covered even though
the town-led sample had suggested 73-79%. A fifth round with new niche and region combinations (all three countries) is running.

| | UK | France | Italy |
| --- | --- | --- | --- |
| Added dealers (live, verified) after round four | 442 | 479 | 369 |

## Round five (niche x region, all three countries, 2026-11-27)
96 new niche-by-region queries (UK 32, FR 32, IT 32; barometers, snuff boxes, Quimper, comtoises, presepi, Deruta ...) found 188 NEW leads; 170 live ones went to 4 qualifier agents: **25 dealers added** (UK 12, FR 7, IT 6).

Coverage of the qualifying dealers in this sample, measured before these 25 were added: **UK 54% (14 of 26, CI 35-71%), France 70% (16 of 23, CI 49-84%), Italy 86% (36 of 42, CI 72-93%)**.
Summary of the independent samples (share of the qualifying dealers found that were already added):

| Sample | UK | France | Italy |
| --- | --- | --- | --- |
| Before the campaign (product-led searches, association frames) | 44-54% | 62% | 75-91% |
| Town-led searches, after waves 1-2 | 73% | 79% | 91% |
| Niche x region searches, rounds 4+5 | 50% (35 of 70) | 62% (38 of 61) | 86% (36 of 42) |

Reading: Italy is nearly saturated; France is mid-way; the UK keeps showing specialist dealers (clocks, arms, tools, maps, silver, jewellery) that no earlier frame reached, so UK coverage of all eligible shops is
probably still around 55-65% and more niche rounds there pay off (about one qualifying dealer per 4-5 leads).

| | UK | France | Italy |
| --- | --- | --- | --- |
| Added dealers (live, verified) after round five | 454 | 486 | 375 |

Tooling note: the UK service code in the shard builder had a space ("UNITED KINGDOM_DELIVERY"); fixed to `UNITED_KINGDOM` in the orchestrator's `mk.py` (no shard contained the bad code).

## Round six (niche x region, UK and France, 2026-11-27)
80 new niche-by-region queries (UK 48, FR 32) found 147 NEW leads; 108 live ones went to 3 qualifier agents: **26 dealers added** (UK 21, FR 5).
Coverage of the qualifying dealers in these samples, measured before these 26 were added: **UK 57% (28 of 49, CI 43-70%), France 83% (24 of 29, CI 65-92%)**.

Trend of the niche-sample recapture (share of qualifying dealers found that were already added):

| Round | UK | France | Italy |
| --- | --- | --- | --- |
| 4 | 48% | 58% | - |
| 5 | 54% | 70% | 86% |
| 6 | 57% | 83% | - |

France is close to the level reached in DACH; Italy is nearly saturated. The UK improves slowly (about 20-25 new dealers per round), so it is still far from complete. Added dealers (live, verified) after round six:
**UK 475, France 491, Italy 375**.

## Round seven (UK niche x region, 2026-11-27)
48 more UK niche queries found 112 NEW leads; 81 live ones went to 2 qualifier agents: **15 dealers added** (7 + 8). About 40% of the leads were US or other foreign sellers.
Recapture on this sample, before the 15 were added: **UK 67% (31 of 46, CI 53-79%)**. UK trend: 48% (round 4), 54%, 57%, 67%. Added dealers (live, verified): **UK 490**, France 491, Italy 375.

## Round eight and closing state (UK, 2026-11-27)
48 more UK niche queries (clocks, furniture, garden/industrial/ephemera) found 106 NEW leads; 81 live ones went to 2 qualifier agents: **12 dealers added**. Recapture on this sample, before those 12: **UK 72% (26 of 36, CI 56-84%)**.
UK trend: 48%, 54%, 57%, 67%, 72%. The campaign stops here: the stop rule (about 75% coverage or fewer than about ten new dealers per round) is nearly met, and each further UK round brings about 10-12 dealers.

### Final numbers (live and verified against the backend)
| | UK | France | Italy |
| --- | --- | --- | --- |
| Added before the campaign | 286 | 460 | 332 |
| Removed as out of policy (audit) | 19 | 50 | 12 |
| Added by the campaign | 235 | 81 | 55 |
| **Added now** | **502** | **491** | **375** |
| Coverage of qualifying dealers (latest niche sample, share already added before that round) | 72% | 83% | 86% |

All 371 dealers added by the campaign are listed with their listing-source ids in `gfi-added.tsv`. Every source was created with empty ingestion; no ingestion was enabled.

### What is open
- **Borderline additions:** many added dealers carry quality flags in the orchestrator's shard notes (price on request under Policy D, locality-only locations, operator = trading name, antique share only estimated at 55-70%). None was removed beyond the audit's clear cases.
- **Audit leftovers:** 25 weaker out-of-policy verdicts and 33 unsure ones from the policy audit stay live (list in the orchestrator workspace, `reports/audit-decision.json`); troc29.fr closes on 31 December 2026.
- **Second storefronts:** a number of dealers were deferred because they are further storefronts of operators that are already live (e.g. athey, antichitabarberia, vendoantiquariato); they need the `existingOperator` flow, not done yet.
- **Deferred pools:** about 170 UK, 470 French and 130 Italian dealers are DEFERRED (no address, no imprint, operator ceased, unreadable site); re-check dates run from 2027-01 to 2027-05.
- **Two French galleries** (galeriedelalande.fr, ratton-ladriere.com) are deferred over their own-publication sections.
