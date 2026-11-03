# Belgium dealer discovery

Owner: Belgium (BE), continuous discovery started 2026-11-03. BE is Belgium’s ISO 3166-1 alpha-2 code, supported by ISO’s Belgium subdivision bulletin (https://www.iso.org/iso/iso_3166-2_newsletter_ii-3_2011-12-13.pdf).

Research and authorized API onboarding continue until stopped. Write only this country directory and manage only its identities. Read all country and nested hosted-dealer ledgers for duplicates; existing ownership takes precedence over geography. Assign new dealers by verified business home country. Foreign leads and proposed enrichment for other owners remain in this frontier.

Evidence and mutation journal: `dealers.jsonl`; research queue and dated follow-ups: `frontier.jsonl`; free search coverage: `searches.jsonl`; generated lookup: `index.tsv`; country live readbacks: `verification.json`. Raw pages, API snapshots and original asset previews are cached under `/tmp/aura-dealer-discovery/BE/`. Credentials remain external.

Import only current first-party facts assessed strictly above 95% confidence. Exclude auction businesses and marketplaces. Keep ingestion empty. Distinguish legal addresses, premises, inventory, dispatch and collection; unknown shipping stays unknown. Full verification initially and at least every fifth batch. Require direct art or antiques relevance; ordinary bookshops and general secondhand retail do not qualify through an incidental item.

## Current checkpoint

be-2026-11-03-055, 2026-11-03T19:50:39.423492Z. No new Belgian API resources in this routing batch. Completed initial triage of all 109 Antica Namur 2026 profiles, preserving open identity, stock and country follow-ups. Verified eight foreign legal homes and queued them to Germany, Italy or France; existing French and Swiss ownership retained. Full verification passed across all four Belgian ledgers: 111 sources, 151 locations and 50 shipping profiles, all ingestion empty. Paid-data spend USD 0; no country blocker. Country coverage remains open. Next full check060.

Verified country totals: {"parties": 111, "listingSources": 111, "locations": 151, "logos": 71, "identifiedPlatforms": 79, "deliveryProfiles": 50, "emptyIngestionMethods": 111, "excluded": 10, "deferred": 131}. All ingestion empty. Paid-data spend USD 0. No country blocker; coverage remains open.

## Continuation

For legal identity gaps, the official Belgian KBO Public Search is a free primary register. Match its active legal entity and full published business address to current dealer evidence; commercial directories and search snippets provide lookup leads only. Register activity alone does not establish current stock.

Resume the frontier; research current first-party identity and stock before mutation. Refresh every country ledger, including hosted ledgers, and run fresh live API identity checks. Use explicit BE ledger and verification output paths. National ROCAD Belgium-filtered directory returned 93 member profiles; profiles 1–93 have been fetched, with websites queued in the frontier. Continue the queued websites and resolve current regional leads. A directory’s country filter does not establish a dealer’s business home. Association/event websites are research sources, not dealers. Raw cursor list: `/tmp/aura-dealer-discovery/BE/rocad-member-links.json`.

All 109 profiles in the 2026 Antica Namur roster now have dated `EVENT_EXHIBITOR_REVIEWED` resolutions in the frontier. This completes initial roster triage; unresolved qualifications and country assignments remain open. Continue the unreviewed merchant URLs from the national, regional and earlier event queues. Resolve confirmed aliases and existing operator ownership before repeating research under another domain.

## Hosted merchant ownership

[Seghers & Pang Fine Arts](hosted/seghers-pang/README.md) has a separate ledger for its exact Anticstore merchant catalogue. The marketplace and unrelated sellers are not owned here. Country `index.tsv` and `verification.json` aggregate the main ledger and verified nested merchants; `ledgerReports` records each explicit verification scope and totals. Run the helper on every ledger with its explicit output path before aggregation; preserve the standard main report externally. Raw snapshots and aggregation helper remain under `/tmp/aura-dealer-discovery/BE/`.

[Antiquaire A. Verelst](hosted/a-verelst/README.md) is also managed through its exact Proantic merchant catalogue. The Proantic platform exclusion in France remains separately owned and unchanged; this ledger never claims the platform or other accounts. Include this nested ledger in every subsequent verification and aggregation.

[Bergmans Scott Fine Arts](hosted/bergmans-scott/README.md) is managed only through its exact Proantic merchant account. Include this nested ledger in all subsequent country verification and aggregation. Other merchants and the platform-root exclusion remain separately owned.
