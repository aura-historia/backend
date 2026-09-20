# Migration F1 — backend contract and survivor inventory

**Baseline:** `dc1ae85af84ee53cf1e8c678e7453017da1ccd56` (`git rev-parse HEAD`, 2026-09-20).

This is the checked-in migration decision reference for [#1775](https://github.com/aura-historia/backend/issues/1775). It records repository declarations, not live AWS state. A resource marked **not verified** needs approved read-only access before a cutover decision. It contains no credentials, provider payloads, or source rows.

## Reading labels

| Label | Meaning |
| --- | --- |
| **Agreed** | Owner decision from migration epic [#1772](https://github.com/aura-historia/backend/issues/1772). Do not reopen here. |
| **Starting default** | Initial candidate from #1772. It is not sizing, cost, or live acceptance evidence. |
| **Implementation decision** | Current checked-in declaration or runtime behavior at the baseline. |
| **Acceptance gate** | Evidence required before replacement, cutover, or cleanup. It is not complete merely because it is listed. |

## Binding scope and exclusions

- **Agreed:** the crawler is self-hosted and outside this workload. Do not migrate or inventory crawler deployment, release, lifecycle, local/durable state, hosting, network, credentials, monitoring, or internals. Do not expose the target private RDS for it.
- **Agreed:** preserve only shared backend ingestion compatibility: crawler-originated raw ProductListing capture must remain compatible with `product_listing_raw_revisions` and normalization. Raw normalization remains an in-scope backend worker.
- **Agreed:** abandoned PR task [#1412](https://github.com/aura-historia/backend/issues/1412), deployment [#1761](https://github.com/aura-historia/backend/pull/1761), is excluded. Its deployment design, branch, tests, runbooks, TLS work, and container recipes are neither baseline capabilities nor acceptance evidence.
- **Agreed:** target architecture is one Axum API Lambda, ten worker Lambdas, private RDS PostgreSQL, committed-row DMS CDC to Kinesis and a router Lambda, scoped Standard SQS source/DLQ pairs, EventBridge Scheduler for bounded backend jobs, one managed NAT/EIP, and external Hetzner OpenSearch. It has no crawler deployment deliverable.
- **Agreed:** preserve private RDS; CDC-only DMS with no outbox/relay/dual-write; no reserved or provisioned concurrency; no unapproved SQS event-source cap. `ReservedConcurrentExecutions` must be absent, including zero.
- **Starting default:** RDS PostgreSQL rather than Aurora; small initial pools; source retention 7d, DLQ retention 14d, redrive 5; one Kinesis shard; slow Lambda batches of one. Recalculate Lambda queue visibility from final Lambda timeout/batching rather than copying native-process values.

## Source authority and known drift

| Area | Authoritative baseline | Non-authoritative or stale evidence |
| --- | --- | --- |
| REST paths and transport behavior | Axum router: `src/aura-historia-api/src/lib.rs` plus nested routers | `docs/swagger.yaml` is the public contract and must be reconciled for implementation; it does not provision a route. |
| REST public contract | `docs/swagger.yaml` | CDK `ROUTES` is empty in `infra/src/constructs/api.ts`; current CDK has no API Gateway route/integration despite creating front-door resources. |
| Native worker scopes/CDC fanout | `src/aura-historia-worker/src/{lib.rs,cdc.rs,jobs.rs}` | `docs/events/flow.md` is the contract summary, not deployment proof. |
| Cloud declarations | `infra/src/constructs/`, composed by `infra/src/application-stack.ts`; synth tests constrain key results | No live account/environment was read. All deployed resource state is **not verified**. |
| Delivery workflow | `.github/workflows/{deploy,integrate,test-images}.yml` | Workflow does not deploy API/worker/cron processes, Sequin subscriptions, or worker identity attachment. |

`docs/events/flow.md` links missing `durable-worker-runbook.md` and `product-listing-raw-normalization-runbook.md`. Those broken links are not operational evidence. `infra/README.md` also describes API routes, IdPs, Pipes, and scheduled Fargate images which current CDK does not declare. These are documentation drift, not evidence that the resources exist.

## HTTP inventory

**Common contract.** Axum applies a 1 MiB body limit and 30-second request timeout. It issues request/correlation IDs and uses redacted tracing. `C` below means Cognito access JWT or Aura opaque access token; supplied invalid credentials fail. Business authorization (administrator, owner, partnership, quota, and delegated capability) is enforced by the service use case, never by a controller. Public `GET`s may use optional valid identity for personalization but do not widen public representation. PostgreSQL is authoritative; OpenSearch is rebuildable search state.

**Acknowledgment.** Normal REST writes acknowledge only after their synchronous use case/transaction outcome. Partner ProductListing writes are synchronous. `POST /api/v1/webhooks/woocommerce/{listingSourceId}` verifies authorization and signature before parsing and returns `204` after the raw-intake outcome (`changed`, `unchanged`, duplicate, stale, or authorized ignored); it does **not** acknowledge canonical normalization. EventBridge provider intake, Cognito triggers, and Sequin are separate from REST below.

| Family | Exact method/path inventory | Authentication and service policy | Body/cache/domain note |
| --- | --- | --- | --- |
| Health | `GET /health`; `GET /ready` | none | `/ready` is `204` only when PostgreSQL and OpenSearch checks pass; otherwise `503`. |
| Product reads | `GET /api/v1/product-listings`; `GET /api/v1/product-listings/{productListingId}`; `GET /api/v1/product-listings/by-slug/{productListingTitleSlugId}`; `GET /api/v1/product-listings/{productListingId}/history`; `GET /api/v1/product-listings/{productListingId}/similar` | public/optional `C`; service owns personalized state | Search reads rebuildable OpenSearch facts plus batched PostgreSQL source/user-state enrichment. Per-process public-search caches only: FX 30s/512; source summary 60s/4,096/8 MiB. |
| Partner listing writes | `POST`, `PATCH`, `PUT`, `DELETE /api/v1/listing-sources/{listingSourceId}/product-listings` | `C`; partnership/admin plus delegated `product-listings:write` | max 100 entries; synchronous PostgreSQL canonical writes. |
| Direct signed webhook | `POST /api/v1/webhooks/woocommerce/{listingSourceId}` | `C` plus WooCommerce signature; service checks ProductListing write permission | raw-only capture; 204 intake acknowledgment described above. |
| Public/admin Auctions | `GET /api/v1/auctions`; `GET /api/v1/auctions/{auctionId}`; `GET /api/v1/auctions/{auctionId}/product-listings`; `POST /api/v1/admin/auctions`; `GET`, `PATCH /api/v1/admin/auctions/{auctionId}` | public reads optional `C`; admin routes `C` + persisted ADMIN policy | PostgreSQL reads; `Cache-Control: no-store`. |
| ListingSources | `GET /api/v1/listing-sources`; `GET /api/v1/listing-sources/by-slug/{listingSourceSlugId}`; `GET /api/v1/me/listing-sources`; `GET`, `POST /api/v1/admin/listing-sources`; `GET`, `PATCH`, `DELETE /api/v1/admin/listing-sources/{listingSourceId}` | public optional `C`; own/admin routes `C` and service policy | public read budget default 4 in flight/500ms; no crawler shutdown or crawler-state transaction. |
| Parties/admin overview | `GET`, `POST /api/v1/admin/parties`; `GET`, `PATCH`, `DELETE /api/v1/admin/parties/{partyId}`; `GET /api/v1/admin/overview` | `C` + persisted ADMIN policy | authoritative PostgreSQL; `no-store`. |
| Account/users/tokens | `GET`, `PATCH /api/v1/me/account`; `DELETE /api/v1/me`; `GET`, `POST`, `PATCH /api/v1/me/access-tokens`; `GET`, `DELETE /api/v1/me/access-tokens/{accessTokenId}`; `GET /api/v1/admin/users`; `GET`, `PATCH`, `DELETE /api/v1/admin/users/{userId}`; `PUT`, `DELETE /api/v1/admin/users/{userId}/suspension`; `POST /api/v1/admin/users/{userId}/sessions/revoke`; `GET`, `DELETE /api/v1/admin/users/{userId}/access-tokens`; `DELETE /api/v1/admin/users/{userId}/access-tokens/{accessTokenId}` | `C`; own/admin and delegated capability checks in User service | Cognito is IdP only; user truth is PostgreSQL. |
| Watchlist | `GET`, `POST /api/v1/me/watchlist`; `PATCH`, `DELETE /api/v1/me/watchlist/{productListingId}` | `C`; owner/quota/lifecycle in service | personalized PostgreSQL read; `no-store`. |
| Saved filters | `GET`, `POST /api/v1/me/search-filters`; `GET`, `PATCH`, `DELETE /api/v1/me/search-filters/{userSearchFilterId}`; `GET /api/v1/me/search-filters/{userSearchFilterId}/matches`; `PATCH /api/v1/me/search-filters/{userSearchFilterId}/matches/{productListingId}` | `C`; ownership and `search-filters:write` delegated capability | authoritative PostgreSQL; projection is asynchronous. |
| Notifications | `GET`, `PATCH`, `DELETE /api/v1/me/notifications`; `PATCH /api/v1/me/notifications/all`; `PATCH`, `DELETE /api/v1/me/notifications/{notificationId}` | `C`; owner/delegated policy | PostgreSQL delivery/provider internals never exposed. |
| Billing/newsletter | `POST /api/v1/me/billing/checkout`; `POST /api/v1/me/billing/portal`; `POST /api/v1/me/billing/manage`; `PUT /api/v1/newsletter-subscriptions` | billing `C`; newsletter public with optional `C` | Stripe/Zoho are separate provider contracts. |
| Partnership applications | `GET`, `POST /api/v1/me/partnership-applications`; `GET`, `DELETE /api/v1/me/partnership-applications/{partnershipApplicationId}`; `GET /api/v1/admin/partnership-applications`; `GET`, `PATCH /api/v1/admin/partnership-applications/{partnershipApplicationId}`; `POST /api/v1/admin/partnership-applications/{partnershipApplicationId}/decision` | `C`; owner/admin and delegated policy in service | decision transaction creates canonical delivery intent; no generic worker route. |
| Partnerships | `GET /api/v1/admin/partnerships`; `GET`, `DELETE /api/v1/admin/partnerships/{partnershipId}`; `PUT`, `DELETE /api/v1/admin/partnerships/{partnershipId}/members/{userId}`; `PUT`, `DELETE /api/v1/admin/partnerships/{partnershipId}/listing-source-grants/{listingSourceId}` | `C` + persisted ADMIN policy | `no-store`; service retains historical dissolution. |
| OAuth | `GET`, `POST /api/v1/admin/oauth-clients`; `GET`, `PATCH`, `DELETE /api/v1/admin/oauth-clients/{clientId}`; `GET /api/v1/oauth/authorize`; `POST /api/v1/oauth/token`; `GET /api/v1/oauth/tokens/by-third-party-code/{thirdPartyCode}`; `POST /api/v1/oauth/revoke`; `POST /api/v1/oauth/introspect` | authorize uses `C`; client protocol endpoints use form client credentials; admin uses `C` + ADMIN/capability policy | secrets returned only once at client creation; never log them. |

**Front-door reconciliation.** `docs/swagger.yaml` and Axum are the route contract/input to migration. The current CDK has an HTTP API, JWT authorizer, custom domain, CloudFront and WAF declarations, but `ROUTES` is empty: it is a surviving front-door resource declaration, not evidence of live API Lambda routing. #1784/#1785 own an explicit route-by-route implementation/reconciliation gate; do not add a global Cognito authorizer or catch-all bypass that breaks opaque tokens or signed webhooks.

## Provider and trigger inventory

| Class | Baseline declaration | Preserve/target owner | Acceptance gate |
| --- | --- | --- | --- |
| Direct signed webhook | WooCommerce REST route above | API Lambda: #1784/#1785; preserve raw-intake compatibility | Signature/raw-capture/204 semantics tested on target. |
| EventBridge provider events | Shopify `products/create|update|delete` bus rule → Shopify SQS → Shopify Lambda; Stripe subscription events → Stripe Lambda | Retained integration: #1798 | Rule/filter, queue/Lambda retry behavior, and source compatibility verified. |
| Cognito trigger | post-confirmation → `cognito-post-confirmation` Lambda | Retained integration: #1798 | User-pool trigger and persisted user mapping verified. |
| CDC provider | PostgreSQL logical replication → externally owned Sequin → `POST /cdc/sequin` native worker | Replace with DMS/Kinesis/router: #1781, #1787, #1788 | Lossless committed-row insert/update/delete/type/delete contract; no live switch assumed. |

## Worker routing and execution inventory

**Implementation decision.** Native router accepts only a 1 MiB/100-change/500-job complete CDC batch. It validates all source rows, schema-2 compact jobs, keys, destinations, and serialized bounds before sending. It returns `202` only after every required SQS send confirms. Partial publication can duplicate on redelivery. Jobs contain typed IDs and domain idempotency/ordering keys, never raw source JSON. Consumers delete a receipt only for a terminal complete outcome; poison, deferred, retryable, timeout, panic, claim ambiguity, and failed delete remain for retry/DLQ.

| Scope and exact source | Schema-2 job / route | Complete outcome | Deferred/retryable/permanent/unknown outcome | Use case, dependency, claim, timing | Target Lambda owner |
| --- | --- | --- | --- | --- | --- |
| `product-listing-opensearch`: `product_listing_events INSERT`, supported domain/enrichment | `ProductListingEventJob`, event/listing IDs; all supported route union | applied, tombstoned/deleted, version-stale | missing source or required sale FX retries; malformed job is poison | `ProjectProductListing`; PostgreSQL + OpenSearch; external projection version; 60s visibility/45s execution | #1783 |
| `search-filter-projection`: `search_filters INSERT|MODIFY|DELETE` | `SearchFilterChangedJob`, filter ID/version/op | target write or stale | invalid source/version poison; read/write failure retries | `ProjectSearchFilterChange`; PostgreSQL + OpenSearch; version tombstone; 60s/45s | #1791 |
| `search-filter-percolator`: `product_listing_events INSERT`, supported domain/enrichment | `ProductListingEventJob`, event/listing IDs | processed, duplicate, stale, inactive/withdrawn, ignored | missing source/FX/provider failure retries; malformed poison | `MatchProductListingEvent`; PostgreSQL, OpenSearch, Vertex; current-event guard/match uniqueness; 300s/240s | #1792 |
| `search-filter-match-notification`: `search_filter_matches INSERT` | `SearchFilterMatchCreatedJob`, exact user/filter/listing/origin-event IDs | created, duplicate, quota, deleted user, stale, withdrawn | missing match/listing retries; malformed poison | `GenerateSearchFilterMatchNotification`; PostgreSQL transaction; exact historical source, tier/quota; 60s/45s | #1793 |
| `watchlist-notification`: ProductListing changed `main-price` or `availability` route | `ProductListingEventJob`, event/listing IDs | applied, duplicate, ignored, withdrawn | missing source retries; malformed poison | `GenerateWatchlistNotifications`; PostgreSQL transaction; historical event time and semantic `(user,event,kind)` key; 60s/45s | #1793 |
| `product-content-assessment`: discovered ProductListing event | `ProductListingEventJob`, event/listing IDs | applied, cleared, duplicate, stale, ignored | missing listing retries; malformed poison | `AssessProductListingContentEvent`; PostgreSQL/system principal/source guard; 60s/45s | #1795 |
| `product-embedding`: discovered or image-changed ProductListing event | `ProductListingEventJob`, event/listing IDs | applied, duplicate, stale, ignored, missing-title no-op | missing listing/provider failure retries; malformed poison | `EmbedProductListingEvent`; PostgreSQL + Vertex/ADC; source-event guard; 300s/240s | #1796 |
| `product-translation`: discovered ProductListing event | `ProductListingEventJob`, event/listing IDs | applied, duplicate, stale, ignored, missing/empty title/language no-op | missing listing/provider failure retries; malformed poison | `TranslateProductListingEvent`; PostgreSQL + Vertex/ADC; source-event guard; 300s/240s | #1797 |
| `product-listing-normalization`: `product_listing_raw_revisions INSERT` | `ProductListingRawRevisionJob`, raw stream/revision/revision number | terminal candidate-data rejection or authoritative stream drain | configuration/persistence and capped continuation retain receipt; malformed poison | `NormalizeProductListingRawRevision`; PostgreSQL UoW, raw progress + canonical event; reconciliation every 30s, max 32/stream, page 100; 300s/240s | #1790 |
| `notification-delivery`: `notification_deliveries INSERT` | `NotificationDeliveryCreatedJob`, delivery ID | delivered, already delivered, persisted permanent failure, finalized source-missing | active lease defers to expiry+5s; missing delivery/lease/ambiguous SES/finalization retries; malformed poison | `DeliverNotification`; PostgreSQL, S3 templates, SES; five-minute lease/four-minute budget; 360s/240s | #1794 |

The exact ProductListing routing union is preserved: discovered → OpenSearch projector, percolator, content assessment, embedding, translation; changed → projector and percolator, plus watchlist for main-price/availability and embedding for images; embedded/translated enrichment → projector and percolator. Combined changed dimensions use the union, never mutually exclusive routing.

**Tables that must not gain a subscriber:** `product_listings`; `users`; `product_listing_watchlist`; `partnership_applications`. `auction_events` has no worker consumer. `product_listing_raw_revisions` routes only normalization. No crawler table or crawler runtime is part of this inventory.

**Current native custody.** Ten Standard source/DLQ pairs are declared: source 7d, DLQ 14d, redrive 5, long poll 20s, SSE/TLS policies; prod retains queue deletion/replacement. All are polling processes, not Lambda event sources. The worker has capacity one/no prefetch, heartbeat `min(30s, visibility/3)`, and jittered retry visibility 30–900s. CDK creates unbound per-scope publisher/consumer policies; the external identity owner attaches them. Native worker/Sequin process deployment is not declared.

## Backend schedules and process-memory assumptions

| Job | Baseline input/owner | State and target | Acceptance gate |
| --- | --- | --- | --- |
| Periodic saved-filter matching | `aura-historia-cron`; `SEARCH_FILTER_PERIODIC_MATCH_CRON`, seven-field UTC, default `0 0 15 * * * *`; max run default 7,200s | Calls `RunPeriodicSearchFilterMatching`; distributed run lock/progress/idempotent matches. Local overlap skip, process scheduler/health/drain are process memory. Target EventBridge Scheduler bounded application Lambda: #1799. | Scheduler expression/timezone, resumability, distributed overlap and target retry evidence. |
| Raw-normalization reconciliation | worker timer every 30s plus reconstructible local FIFO/cursor | It repairs missed raw CDC wake-ups but local continuation is not durable custody. Target bounded resumable schedule: #1799; worker #1790. | Authoritative traversal works after process death and no timer cadence is treated as correctness. |
| FX initial capture | compute-stack custom resource invokes FxRate Lambda with stable deployment source ID | PostgreSQL immutable snapshot; target scheduler/application Lambda work: #1799/#1798. | Initial capture runs after approved schema migration, is idempotent, and failure behavior is explicit. |
| FX recurring capture | EventBridge `cron(0 6,18 * * ? *)`, real stages only; max event age 1h, retries 3 | FxRate Lambda; no native cron process | Preserve as bounded scheduled work; verify final schedule/retry/identity under #1799/#1798. |

Do not migrate crawler schedules, lifecycle, or local state. The listed local timers, in-process overlap flags, health process state, cache contents, worker receipt ownership, and native scheduling are migration hazards: target correctness must live in PostgreSQL/DMS/SQS/Scheduler ownership, not process memory.

## Resource and workflow ownership matrix

All rows are **not verified live** unless stated otherwise. “Declared” means source code only. Target owner is the migration task that must make the replacement/cutover decision; it does not authorize deletion.

| Resource/workflow | Baseline declaration; environment/account/region | Retained state and callers/producers | Replacement owner | Preservation/cutover gate | Legacy cleanup owner |
| --- | --- | --- | --- | --- | --- |
| Business PostgreSQL, schemas, logical slots/WAL | External connection settings only; no RDS/VPC/DMS CDK resource. Account/region not verified. | Authoritative business tables, events, raw revisions, leases, tokens; API/Lambdas/workers/cron/Sequin | #1776–#1779, #1781 | Private RDS, supported schema/TLS/identities, backup/recovery and slot/WAL proof; no crawler DB migration | #1806 after retained CDC/cutover evidence |
| DMS/Kinesis | No current declaration; Sequin is external | Committed row source; router consumer | #1781, #1787, #1788 | Lossless schema/type/delete/checkpoint/replay evidence; DMS is CDC-only/no outbox | #1806; never drop slots automatically |
| Native worker queues/DLQs/messages | Declared per stage in CDK; account/region not verified | In-flight jobs/DLQs; Sequin router and native worker | #1783, #1790–#1797 | New scoped queue/Lambda preserves keys, guards, terminal-only acknowledgment, and message retention; drain/capture legacy queues/DLQs before stop | #1806 approved operator, never runtime purge |
| API/runtime processes | Source declares `aura-historia-api`, worker, cron; no CDK deployment | REST clients, provider callbacks, process-local cache/timers | #1780, #1784, #1785, #1799 | One API Lambda and bounded jobs pass route/provider contract gates; no broad auth bypass | #1806 |
| OpenSearch indexes | Real endpoint external; ephemeral LocalStack domain; account/region not verified | Rebuildable `product-listings` and `user_search_filters`; API/workers | #1782, #1803 | Secured endpoint, fenced rebuild, mappings/readers before writers, tombstones retained | #1806 only after rebuild/rollback gate |
| Cognito/user integrations | CDK user pool/client/domain/post-confirmation; real state not verified | Identity provider and user mapping | #1798 | Preserve issuer/audience/JWKS/custom-token behavior and trigger mapping | #1806 coordinate owner |
| Shopify/Stripe/EventBridge/SQS | CDK rules; Shopify queue/Lambda mapping; real state not verified | External provider events/raw capture/subscription facts | #1798 | Preserve filters, signed/provider contract, retry and idempotency; coordinate producers | #1806 coordinate owner |
| SES/templates/artifact/template stores | Fixed S3 imports/CI uploads; real bucket/account state not verified | Lambda ZIPs, compiled email templates, delivery rendering | #1780, #1802, #1794 | Immutable promotion/rollback, scoped read/send access; no secret/template payload exposure | #1806 |
| IAM roles/policies | Lambda roles implicit; native 20 unbound policies; real attachments not verified | CI deploy, Lambda execution, external worker identity | #1777, #1780, #1783 | Scoped execution roles and credential refresh; no reserved concurrency/operator powers | #1806 with identity owner |
| API Gateway/domain/CloudFront/WAF/DNS | CDK front-door resources; real DNS/distribution/WAF not verified; CloudFront WAF is `us-east-1`, target workload region agreed `eu-central-1` | Public API aliases, TLS, WAF/cache policy | #1784, #1785; coordinate #1300 | Route parity, custom credentials/webhooks, cache/auth behavior, domain/TLS/DNS reviewed before traffic switch | #1806 with DNS/front-door owner |
| Network/NAT/EIP | No VPC/NAT/security-group declaration | External connectivity currently unknown | #1777 | Two-AZ isolated VPC, one managed NAT/EIP, private RDS, exact endpoint choices; no crawler connectivity scope | #1806 |
| Observability/dashboards | Prod SQS age/DLQ alarms declared; other worker signals logs; no dashboard declaration | Alarms/log groups; consumers/operators | #1804; coordinate #1015 | Alarm/metric/cost visibility evidence; do not infer a dashboard exists | #1806 |
| CDK/GitHub deployment workflow | CDK change sets plus Lambda/template artifact upload; no API/worker/cron/Sequin deployment | CommitSHA artifacts and CI | #1802 | Immutable build/promotion/alias rollback; approved migrations separate from code release | #1806 |

## Cross-task decisions and gates

| Open decision | Assigned migration task(s) |
| --- | --- |
| Engine/toolchain, exact RDS resources, scoped identities, recovery | #1778, #1780, #1781, #1782, #1786 |
| DMS endpoints, Secrets Manager path, deletes/types/LOBs, checkpoint/replay window | #1781, #1787, #1788, #1789 |
| Lambda batch/visibility/deferred-claim behavior | #1783 and each worker; especially #1794 |
| TTL cleanup/cadence | #1776, #1799 |
| API routes, webhook/custom auth, front door/domain | #1784, #1785, #1798; coordinate #1300 |
| Package/credential refresh/aliases/migration runner | #1779, #1780, #1802 |
| Search sizing, install, fenced rebuild/recovery | #1782, #1803 |
| Measured uncapped capacity and later mitigation | #1786, #1805; #449 reserved-concurrency instruction is superseded |
| Cost, backlog, visibility and dashboards | #1804; coordinate #1015 |
| Final backend legacy retirement | #1806 only after #1805 evidence |

Coordinate existing #1300 (CloudFront), #449 (production config; its reservation instruction is superseded), and #1015 (LLM observability) only where they affect these gates. Crawler-related #1359, #784, and #1583 stay untouched, outside scope, and are not prerequisites.

## F1 acceptance checklist

- [x] Baseline SHA, current code declarations, route families, ten scopes, schedules, and workflow gaps are recorded.
- [x] Crawler exclusion, #1412/#1761 exclusion, no-outbox CDC, private RDS, one API Lambda/ten workers/one NAT, and concurrency restrictions are explicit.
- [x] Live state is distinguished as **not verified**; no secret or full payload is recorded.
- [ ] Replacement tasks must use this inventory to prove individual resource/contract gates.
- [ ] #1805/#1806 must collect live, authorized acceptance/cutover evidence before any legacy cleanup.
