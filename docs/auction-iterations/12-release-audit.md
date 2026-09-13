# Iteration 12 — Development-release audit

**Gate: INCOMPLETE. Do not release.** Audited 13 September 2026.

## Objective and checkout

Prove the assembled discovery/cataloguing feature, current contracts, and clean development initialization. No bidding, results, sessions, attribution, reminders, migration tool, or additional production capability belongs here.

- Starting and final HEAD: `23186a188c3da6a54f42209b86de6f7b96d84104` (iteration 11).
- Starting worktree: clean. Result: uncommitted audit documentation and private PostgreSQL regression tests; no commit created.
- Prerequisite: [iteration 11](11-auction-membership-search.md), then all 00–10 records. Their PASS headings were checked against commands, source, and git history, not accepted as release proof.
- Entry prerequisite **not satisfied**: required functionality/acceptance evidence is missing. This record reopens owning gates; it does not silently repair production code in a catch-all integration change.

## Release blockers and owning fixes

### A12-01 — RESOLVED: correction SQL on real PostgreSQL (07)

The audit found Rust backslash-newline continuations that dropped SQL whitespace. The repair replaces every affected statement with explicit raw SQL:

- First activation, lines 87–91: `DO NOTHINGRETURNING`.
- Reactivation, lines 100–103: `product_listing_auction_overridesSET`.
- Release, lines 137–140: joined table/keyword and `TRUERETURNING`.
- Floor insertion, lines 165–179: additional joined tokens, including `generationFROM`, `headJOIN`, and `UPDATESET`.

Four private adapter tests call the real factory/ports on fresh harness PostgreSQL. They assert successful activation/reactivation/release and lock serialization:

| Regression | Observed result |
| --- | --- |
| `should_activate_version_one_when_auction_override_is_absent` | PASS — creates active version one and restricted correction audit. |
| `should_reactivate_and_increment_version_when_auction_override_was_released` | PASS — updates version two to active version three. |
| `should_store_capture_and_stream_floors_when_releasing_override_with_linked_raw_stream` | PASS — records global generation, release audit, and pending linked-stream revision floor. |
| `should_block_a_second_policy_decision_until_the_first_transaction_commits` | PASS — a second transaction cannot acquire the same policy lock before the first commits. |

The release fixture captures two valid current revisions, binds the stream with revision one processed, and proves the release floor includes pending revision two. Seeds validate the current discovered-event codec and typed raw normalizer. The tests remain enabled. Focused command: `cargo test -p product-listing-postgres --lib --all-features product_listing_auction_override::tests:: -- --nocapture` — 4 passed.

The SQL adapter blocker is closed. The policy-only correction/release HTTP flow is now covered separately; raw/partner/admin race acceptance remains A12-04.

### A12-02 — RESOLVED: policy-only correction fencing (07)

The audit found this interleaving:

1. Raw/partner write reads absent/inactive override policy.
2. An equal-context administrative correction activates policy without changing the listing version.
3. The previously admitted writer persists a context change against that still-valid listing version.

The repair adds `ProductListingAuctionOverrideRepository::lock`. PostgreSQL uses `pg_advisory_xact_lock(hashtextextended(listing UUID text, 0))`, which survives for the caller transaction and serializes an absent or persisted policy row. Correction and release acquire it **before** loading/checking expected listing state. Canonical raw writes acquire it before policy reads; direct typed partner resolution does the same. Thus a writer admitted before correction either commits before correction, causing correction's post-lock listing-version check to conflict, or waits and sees the active barrier. Policy-only writes retain independent policy versions and do not invent listing events.

The fourth real PostgreSQL adapter test demonstrates the transaction wait. A separate black-box API test proves the policy-only correction/release flow. Full raw/partner/admin race acceptance remains A12-04.

### A12-03 — RESOLVED: OpenAPI parsing and Auction response contracts (06/09/10/11)

The undefined `id003` alias is now anchored at its strict ListingSource ID schema. `python3 -c 'import yaml; yaml.safe_load(open("docs/swagger.yaml"))'` passes, as does a component-reference check.

The repair separates partner-write `ProductListingAuctionData` (`sourceAuctionId` and fill-only `metadata`) from safe public/history `ProductListingAuctionContextData` (`auctionId`, lot fields, timing only). Detail and history now reference the safe type. Public directory/detail/catalogue `200` responses now have DTO-shaped component schemas and unambiguous quoted descriptions. The OpenAPI update changes no API route, version, or compatibility alias.

YAML/component validation proves the documentation artifact and internal refs. HTTP DTO conformance continues under A12-04.

### A12-04 — Acceptance and source coverage incomplete (06–11)

Existing broad suites do not establish the requested Auction scenarios:

- 06 records integration compilation with `--no-run`, not execution of a reliable-ID typed partner HTTP acceptance case. Audit did not establish real 100-listing grouping/concurrent-first-discovery/rollback acceptance coverage.
- 07 has successful correction/release SQL and a policy-only real HTTP flow, but lacks full raw/partner/admin race coverage.
- 08 has one checked-in Lot-tissimo HTML source fixture and a separate **synthetic** `example.test` raw-to-normalizer test. That is not HTML → capture → worker → canonical acceptance. The requested two-lot, name-only, misleading banner, extended timed deadline, and live-start-not-lot-close fixture matrix is not established. No broad live-source coverage claim is justified.
- 09 batches summaries in code, but response/schema conformance and full boundary coverage remain incomplete.
- 10 HTTP coverage exercises an empty anonymous catalogue. Populated/personalized image redaction, withdrawal visibility, and query-count evidence are not proved by that test; SQL-reader tests cover only their own boundary.
- 11's existing full percolator suite does not establish an Auction-specific saved-query/membership-correction match flow. Query-JSON predicate assertions do not substitute for persisted saved-search/worker acceptance.

These are missing required work/evidence, **not deferred product features**. Close each owner before rerunning 12.

### A12-05 — Coordinated reset/recapture rehearsal missing (12 and persisted-contract owners)

Fresh test PostgreSQL initialization succeeded and existing API tests initialized their harness stores. This is **partial local initialization evidence only**. No coordinated business schema/index/selector/provider receipt/queue reset and recapture rehearsal was performed. No approved shared reset command or named environment authorization was supplied/discovered. Do not invent a destructive command or mark this gate PASS.

## Prior handoff audit

“Recorded” below preserves historical command reports; it does not certify missing acceptance or prove an unchanged environment.

| Iteration / committed snapshot | Recorded evidence and current audit status |
| --- | --- |
| 00 / `5e8836d26` | Planning, format/check/graph recorded PASS. No behavior change. |
| 01 / `e3ae2964d` | 27 pure tests; format/check/graph/lint recorded PASS. Core suite passes again here. |
| 02 / `73e4366e0` | Focused service/SQL tests recorded. Own snapshot left workspace gate pending; 03 commit amended its closure. Current Auction SQL suite passes, but original isolation ordering is not proved. |
| 03 / `9d89db692` | Admin HTTP, Auction suites, workspace libraries/lint recorded PASS. |
| 04 / `2cd8dc276` | Both price formats/producers/raw paths and broad gates recorded PASS. No historical raw decoder found. Coordinated reset evidence still absent. |
| 05 / `2cb1dbf0a` | Library/SQL/OpenSearch/crawler tests recorded PASS. Owning HTTP/worker acceptance execution not recorded; retain verification gap. |
| 06 / `1f9f8fea3` | Recorded PASS reopened: A12-03/04; `--no-run` was not runtime acceptance. |
| 07 / `80cbc2315` | **INCOMPLETE**, A12-01/02/04. Own snapshot had worker failures/timeout; 08 commit repaired worker code and amended 07 to PASS. That does not prove isolated closure before 08 began. |
| 08 / `dcdce895b` | **INCOMPLETE**, prerequisite 07 and source acceptance A12-04. Recorded crawler/worker tests remain historical evidence. |
| 09 / `1e05261d2` | **INCOMPLETE**, prerequisite and response contract A12-03/04. |
| 10 / `d84939040` | **INCOMPLETE**, response schema/populated acceptance A12-03/04. Query-count probe explicitly not run. |
| 11 / `23186a188` | **INCOMPLETE**, OpenAPI parse/acceptance A12-03/04 and prerequisites. Earlier OpenSearch race skips were superseded by recorded unfiltered runs; no outstanding skip waiver inferred. Own handoff omitted final workspace-library/lint results; this audit ran them. |
| 12 / uncommitted diff | **INCOMPLETE**, blockers above. No release approval or next iteration. |

Do not rewrite history to say 02/07 were independently green at their original commits. A later run cannot establish that historical fact.

## Commands actually run

Linux, repository Rust toolchain, local Docker, pinned cached PostgreSQL/LocalStack images; required LocalStack credential was available, never printed. Established process-isolated harness setup/teardown only.

| Command | Result / boundary |
| --- | --- |
| `git --no-optional-locks status --short`; `git --no-pager rev-parse HEAD` | Clean entry; SHA above. |
| `cargo fmt --all -- --check` | PASS at entry and after initial regression additions. |
| `cargo depgraph-check check` | PASS at entry and after initial regression additions. |
| `cargo check --workspace` | PASS at entry and after initial regression additions. |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::result-large-err` | PASS on baseline and after initial regression additions. Baseline 600-second bound; subsequent combined checks 120-second bound. |
| `cargo test --workspace --lib --all-features` | **Baseline PASS**, 3,057 passed, zero failed/ignored/filtered, 73 binaries, 346.84 seconds within 600 seconds. Run before new regressions; not a final-diff PASS. |
| `cargo test -p auction-core --all-features` | PASS, 27 tests. |
| `cargo test -p auction-service --all-features` | PASS, 13 tests. |
| `cargo test -p auction-postgres --all-features` | PASS, 8 library + 3 integration tests. |
| `cargo test -p aura-historia-api --all-features` | Run twice, 600-second bounds, no timeout. Second run completed successfully: 319 acceptance tests passed; library aggregate summary not retained. Test-only adapter additions were present during compilation; no production code changed. Do not invent full API totals or call this correction acceptance. |
| `cargo test -p product-listing-postgres --lib product_listing_auction_override::tests:: -- --nocapture` | FAIL, three regressions, SQLSTATE `42601`. |
| `cargo test -p product-listing-postgres --lib --all-features product_listing_auction_override::tests:: -- --nocapture` | FAIL after coherent fixture refinement: same three errors, zero ignored. |
| `python3 -c 'import yaml; yaml.safe_load(open("docs/swagger.yaml"))'` | FAIL, undefined alias `id003`, line 13417. A preceding response-inspection script failed at the same parse boundary. |
| `git --no-pager diff --check` | PASS after test edits. |
| `cargo test -p product-listing-postgres --lib --all-features product_listing_auction_override::tests:: -- --nocapture` | PASS after SQL repair: 4 passed. |
| `cargo test -p aura-historia-api --test api --all-features should_correct_and_release_product_listing_auction_context_without_public_domain_changes` | PASS: 1 black-box policy-only correction/release flow. |
| `python3 -c 'import yaml; yaml.safe_load(open("docs/swagger.yaml"))'` | PASS after OpenAPI anchor/schema repair. |

The earlier whole-workspace rerun stopped at the then-failing A12-01 regressions. It remains historical evidence, not the current result. The source-key digest was subsequently tightened to the actual SHA-256 of the fixture key. The explicit SQL repair, transaction-scoped policy lock, OpenAPI repair, and focused HTTP acceptance are now present. Final complete gates must still be rerun after all owning acceptance work.

No command above timed out. Targeted test filtering selects regressions; none is disabled/ignored. The release gate remains incomplete because the required wider acceptance and authorized reset rehearsal are unverified.

After the repair and the added black-box test, the current diff passed `cargo fmt --all -- --check`, `cargo depgraph-check check`, `cargo check --workspace`, `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::result-large-err`, and `cargo test --workspace --lib --all-features`. The complete `cargo test -p aura-historia-api --all-features` suite also passed: 320 tests, zero failures, in 225.73 seconds. `python3 -c 'import yaml; yaml.safe_load(open("docs/swagger.yaml"))'` and `git --no-pager diff --check` passed. These gates prove the repaired current paths; they do not replace the missing acceptance scenarios.

Not run in this audit: full worker/CDC black-box acceptance, every `/tests` target for all affected crates, complete section-17 matrix, and coordinated reset rehearsal. These are **unverified**, not waived. Workspace library run covers existing affected library consumers (including provider, notification, watchlist, raw and OpenSearch libraries), not all their black-box targets. Infra test/synth not run: this diff changes no infrastructure/wiring. Rerun full section-18 checks after owning fixes.

## Acceptance accounting

| Required scenarios | Evidence and remaining limit |
| --- | --- |
| ID-01–09, ID-11–12; TIME core/policy cases | Current Auction core/service/adapter suites pass. No claim that pure tests prove memberships or concurrent SQL discovery. |
| ID-10; MEM-01–09/17–18; ING-01–18 | Existing library/adapter evidence and current codecs inspected. Full grouping, raw rollback/replay, source isolation, typed partner and worker acceptance matrix remains unverified; A12-04. |
| MEM-10–16/19; API-21 | Real SQL regressions and policy-only correction/release HTTP acceptance pass. Full raw/partner/admin race coverage remains A12-04. |
| TIME-01–17 | Existing core/normalizer/reader tests pass at baseline; seven source fixture cases and whole runtime matrix not established. TIME-18 conditional local-day helper not claimed implemented/tested. |
| API-01–20 | Existing API/reader/query suites pass within their tested scope. OpenAPI parses; populated catalogue, source-price guard and Auction-specific matching/privacy coverage still need owning verification. |
| DEV-01–04/06–08 | Static cleanup/fence inventory below; no universal PASS because required consumer acceptance remains incomplete. |
| DEV-05/09 | **INCOMPLETE**: owning isolation evidence gaps and coordinated initialization/recapture not proved. |
| DEV-10 | No shared reset, deploy, push, merge, remote purge, issue closure, or unauthorized action. |

## Contract and architecture audit

Searched current tracked source/docs/migrations/mappings/infra for raw version-suffixed types, old start/end fields, compatibility dispatch, transition migrations, ignored tests, stubs, and reverse dependencies. Reviewed targeted serializers, repositories, readers, routes, CDC, projection, and manifest graph. This is scoped evidence, not a proof every invariant holds.

- One strict `ProductListingRawValues`; required `priceFormat`; only raw discriminator `1`. No retired V1/V2/V3 decoder found. Both current price formats remain.
- Auction/listing journals remain schema `1`; worker SQS envelope remains its separate existing `2`. Current normalizer algorithm marker is `4`; it is not another accepted raw-shape decoder. No marker changed in this audit.
- No active ambiguous Aura auction start/end alias found. Rejection tests, historical documentation, and provider-native HTML labels are not compatibility readers.
- One initial business migration remains; no Auction transition migration, dual writer, new index alias, or feature flag added here.
- No `todo!`, `unimplemented!`, `NOT_IMPLEMENTED`, or ignored-test attribute found in scanned source. Real handlers and the repaired SQL paths execute in focused PostgreSQL tests.
- No normal-dependency path from Auction core/service back to ProductListing core/service. Dependency graph passes. New tests remain private; no visibility widened.
- Auction and listing aggregate CAS, raw per-stream revision order/generation, event IDs/current-source checks, OpenSearch external versions and withdrawal tombstones remain. A transaction-scoped listing advisory lock now serializes policy-only correction/release decisions with ordinary raw/partner Auction decisions; wider race acceptance is still unverified.
- Shared Auction metadata stays outside listing search documents. Summary hydration batches unique IDs; catalogue SQL is bounded. No measured query-count/latency claim.
- Auction events have no projection consumer; existing test Sequin table routing includes listing journals/raw revisions, not Auction journals. Deployed external CDC configuration is unverified.

## Source coverage

`src/crawler/tests/fixtures/html/lot-tissimo_listed.html` is a checked-in source HTML fixture, not a new live crawl. `scraper/auction.rs` tests catalogue `leipzig10033`, name `Auktion 9`, lot `54`, and rejects four unproven URL cases. This audit did not verify capture permission/provenance or live site behavior.

`product_listing_raw_normalization`'s crawler-shaped example uses synthetic `example.test` data and invokes the real normalizer/resolver, not the HTML extractor or running worker. The new override fixtures are explicitly synthetic and use no live provider. Ordinary Shopify/WooCommerce remain non-auction producers absent explicit evidence. Unsupported/name-only/unqualified evidence is not reliable identity or an invented deadline. The complete required seven-fixture coverage remains A12-04; do not label all auction sites supported.

## Current storage and development rollout hold

This audit changes **no** schema/raw/API/event/index contract. Earlier direct replacements require a matching disposable dataset:

| Owner | Affected retained/current state |
| --- | --- |
| Auction | `auctions`, `auction_schedule_points`, `auction_events`, `auction_metadata_policy_audits`, `auction_metadata_field_protections` |
| Listing | `product_listing_auction_contexts`, `product_listing_lot_auction_timings`, current listing state/journal, override/correction/release/floor tables |
| Ingestion | Raw streams/revisions/normalization heads/results and Auction evidence/diagnostics, capture generations, provider receipt/deduplication/order state |
| Search | Current ProductListing documents/mapping/tombstones; saved-filter PostgreSQL JSON and `user_search_filters` OpenSearch projection |
| Crawler/runtime | Current selector fixtures/local schema, raw capture payloads and affected queued jobs; unchanged common envelope markers |

Available tooling: `test-api::Postgres::new("migrations")` provisions pinned `pg_ttl_index` PostgreSQL and applies schema once per process; teardown truncates application data, not a schema migration. OpenSearch harness provisions mappings/indexes once and clears canonical documents. `WorkerSqs` owns only its PID-isolated queue pair. Crawler `scripts/linux/db-reset.sh` and Windows counterpart own crawler-local Compose state, **not** a coordinated backend reset. Their presence is not authorization to run them against another environment.

Required authorized rehearsal, after fixes:

1. Name/verify the disposable environment, permissions and exact reviewed reset tooling. Stop API intake, crawler, Shopify/WooCommerce intake, worker, cron and affected clients.
2. Isolate old incompatible messages before rebuilding stores. Inventory provider receipts/source-order heads so they cannot suppress authorized recapture. Do not delete unrelated users/services/identity providers.
3. Use the harness for a fresh test environment, or separately approved environment tooling. Recreate schema in its checked-in FK order (sources before Auctions; listings before listing contexts/policy; raw streams/revisions before linked floors); use existing deferred journal/root constraints. Do not improvise table-by-table shared deletes.
4. Initialize current OpenSearch mappings/documents and saved filters, regenerate only authorized incompatible crawler selectors, seed coherent source/listing/Auction identities, and recapture current typed raw inputs.
5. Start targets/queues before Sequin; run matching worker HTTP before watched commits as required by `test-api`. Start all producers/consumers and clients from one checkout. Verify grouping, correction/release, catalogue, search, and replay smoke tests.
6. Record executed commands, environment and results. Until then DEV-09 remains unverified.

Rollback requires the complete earlier checkout **and its compatible disposable schema/index/queue/selector/fixture set**. Binary-only rollback is unsupported. No migration, reinterpretation of old dates, raw JSON rewrite, or legacy reader is an alternative.

## Handoff

Changed family: `product-listing-postgres` SQL/lock repair and private regressions; `product-listing-service` transactional policy locking; API black-box correction/release acceptance; OpenAPI schemas; owning documentation/handoff status. No dependency, schema, route, or compatibility-version change. Current-doc stale iteration statements corrected.

Next action is **separate owning-iteration acceptance**: close 06/08–11 coverage gaps, especially source fixtures, raw/partner/admin race, populated catalogue, and persisted saved-search flow, then rerun 12. The authorized reset rehearsal also remains required. No next implementation iteration is authorized by this failed gate. Genuine deferred product work remains sessions, auction-level authoritative feeds, offering occurrences, prices/results, reminders, attribution/location/global identity, and richer search projection. None substitutes for the required missing work above.
