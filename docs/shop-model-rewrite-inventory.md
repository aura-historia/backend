# Shop model rewrite: iteration 0 inventory and iteration 1 Party slice

## Record

- Starting commit: `e03104c38f32a8155430ba3c3f84771581d56672` (`task(#1642): prohibited-content revision (#1648)`).
- Branch: `epic/#1437-shop-model-rewrite`.
- `develop` and `origin/develop` were the same starting commit. Fetch succeeded; no rebase was needed.
- Iteration 0 changed documentation only. Iteration 1 adds the isolated Party vertical slice below; it does not change existing Shop consumers or begin cutover.

## Iteration 3 Partnership slice

- `partnership-core`, `partnership-service`, and `partnership-postgres` add a Party-scoped partnership application flow without changing the legacy Shop partner flow.
- Submitted applications retain an existing ListingSource reference or a validated proposed Party/ListingSource payload. They create no draft Party or source.
- Approval atomically creates proposed Party/ListingSource state when needed, finds or creates the Party partnership, and idempotently grants membership plus ListingSource access. Approval and rejection atomically create canonical applicant notification and EMAIL delivery intents through the notification factory. ListingSource grants are source-scoped; no new flow reads or writes Shop partner status.

## Iteration 2 ListingSource slice

- `listing-source-core`, `listing-source-service`, and `listing-source-postgres` add an isolated listing acquisition source owned by a Party.
- The source has stable slug/name, acquisition methods, presentation/referral behavior, and provider-specific PostgreSQL configuration. Provider secrets stay adapter-private.
- New Party operators and ListingSource persist atomically in one service-owned PostgreSQL transaction. Existing Shop flows and runtime composition remain untouched.
- Party-based provider/grant ports are intentionally unwired: old grants bind users to legacy Shops, not Parties. Composition-root cutover is Iteration 5 after the Party grant model exists; Iteration 2 must not couple ListingSource back to Shop.

## Iteration 1 Party slice

- `party-core`, `party-service`, and `party-postgres` establish Party identity, stable slug, name, and optional phone/email contact.
- PostgreSQL `parties` is authoritative and uses optimistic `version`; aggregate state excludes timestamps and version.
- Party create, update, and internal details use cases require the existing service-layer admin-or-internal policy and service-owned transactions.
- No API route, Shop consumer, event, projection, Party role/type/lifecycle/merge/address, or Shop-to-Party cutover is included.

## Scope lock

**Target:** establish the complete impact boundary for a later `Shop` model rewrite while preserving `Shop`, `ProductListing`, and `PartnerShopApplication` as separate bounded contexts until a later approved design changes that rule.

**Non-goals:** implement a model, rename active contracts, alter tables/migrations, change routes, change OpenSearch mappings, change provider/crawler behavior, add/remove crates or dependencies, or revise historical changelog text.

## Terminology lock

- Aura-owned catalog aggregate is `ProductListing`, never `Product`. Its IDs are `ProductListingId`, `ProductListingSlugId`, `ShopListingId`, and `ProductListingKey`.
- `Shop` owns shop identity, integrations, presentation, contact/address, lifecycle, partner status, and affiliate configuration.
- `PartnerShopApplication` is a separate aggregate. Its payload references a `ShopId` for an existing or new shop; it does not hydrate a `Shop`.
- Provider and source vocabulary remains local: Shopify product payloads/topics, WooCommerce product terms, schema.org `Product`, crawler extraction models, fixture copy, and dated changelog history may retain `product`.
- The scan found active human-facing `Product` wording in `docs/swagger.yaml`, plus product-named API and crawler files. A later iteration must classify any wording it touches against `docs/product-listing.md`; it must not reintroduce a `Product` core contract or `ProductState`.

## Impact inventory

### Canonical domain types

- `src/shop-core`: `Shop`, `ShopId`, `ShopSlugId`, `ShopName`, `ShopType`, `ShopLifecycle`, `ShopPartnerStatus`, `Domain`, `ShopifyIntegration`, `WoocommerceIntegration`, presentation, address/contact, and affiliate values.
- `src/shop-partner-core`: `PartnerShopApplication`, ID, state, decision, and existing/new-shop payload.
- `src/product-listing-core`: `ProductListing` references `ShopId`, `seller_id`, and `ShopListingId`; `ListingAvailability`, `ListingOrderability`, `ListingLifecycle`, and sale observation remain canonical listing state.

### Service contracts

- `src/shop-service`: create/update/publish-facing shop commands; partner-status and membership commands; get/search/list/check queries; repository, details/search, partner, and WooCommerce ports.
- `src/shop-partner-service`: application create/list/get/withdraw plus admin list/get/review/decision contracts; application, membership, and user-partner-shop ports.
- `src/product-listing-service`: create/update/upsert/withdraw, provider ingestion, event projection, translation, embedding, assessment, watchlist notification, detail/history/search/similar contracts. Its ports need shop/seller facts in listing views, search, provider authorization, and downstream events.
- Transactional writes must remain service-owned and use shared `application` unit-of-work contracts plus transaction-scoped factories; no controller, crawler, or Lambda may own cross-context business orchestration.

### Repositories and readers

- `src/shop-postgres`: `ShopRepository`, `PartnerShopRepository`, shop details/search, partner membership, and WooCommerce webhook/signature readers.
- `src/shop-partner-postgres`: partner application and user-partner-shop membership repositories; application and user-partner-shop readers.
- `src/product-listing-postgres`: listing repository/event store plus listing detail, batch, user-state, watchlist, match-source, revision-guard, translation, embedding, and assessment adapters. Listing read models join shop/seller facts without leaking SQL rows.
- `src/search-filter-postgres`, `src/watchlist-postgres`, and `src/notification-postgres` consume listing IDs/facts through focused readers and writes; they are affected only where a changed shop model changes a listing-facing read model.

### SQL

- `migrations/20260725090000_initial_business_schema.sql` owns `parties`, `shops`, `user_partner_shops`, and `partner_shop_applications`; Party has stable unique slug, optional phone/email contact, and optimistic version. Shop lifecycle, partner, integration, version, foreign-key, and application-state constraints remain material.
- `product_listings` has required `shop_id` and `seller_id` foreign keys to `shops`; its unique `(shop_id, shop_listing_id)` key makes shop identity part of listing identity.
- Related material tables: `product_listing_events`, translations, assessments, `product_listing_watchlist`, `search_filters`, `search_filter_matches`, `notifications`, and `notification_deliveries`.
- Any later schema edit must review Sequin consumers and use expand/contract where practical. PostgreSQL stays authoritative; no synchronous OpenSearch write joins the SQL transaction.

### API

- `src/aura-historia-api/src/shops`, `partner_applications`, `partner_product_listings`, and `product_listings` hold affected controllers and DTOs. `AppState` wires separate Shop, Partner Application, ProductListing, Watchlist, Search Filter, and Notification inbound bundles.
- Public contract hits include `/api/v1/shops`, `/api/v1/shops/{shopId}`, `/api/v1/shops/{shopId}/product-listings`, and `/api/v1/product-listings/{productListingId}` with history/similar/search routes in `docs/swagger.yaml`.
- API remains thin: map REST input/authentication to service commands and `OperationContext`; do not expose adapter types or compose reads in controllers.

### OpenSearch

- `opensearch/mappings/shops.json` is the strict shop projection schema: IDs/slugs, name/type, domains, presentation, address/contact, specialities, partner status, and audit fields.
- `src/product-listing-opensearch` owns private listing documents, projection writes, listing search/similar readers, and percolation JSON. `opensearch/mappings/product_listings.json` includes shop/seller IDs, slugs, names, and type.
- `src/search-filter-opensearch` and `opensearch/mappings/user_search_filters.json` hold shop/seller/type filter fields and private `priceByCurrency` percolation data.

### Crawler

- `src/crawler` owns source extraction, review data, normalization, availability mapping, candidate/shop registration, and `ProductListingPushService` batching/coalescing into `UpsertProductListingUseCase`.
- Crawler `Product` names are boundary-local. It must preserve explicit `Availability`, `NoAssertion`, and `Ignore` semantics; only verified removal evidence withdraws a listing.

### Provider integration

- `src/shopify-lambda` maps Shopify events into `IngestShopifyProductListingUseCase` or `WithdrawProductListingUseCase` after a shop lookup and partnered-status check.
- WooCommerce enters through API webhook routes and shop-service webhook/integration ports. Shopify and WooCommerce integration fields are current `Shop` aggregate state.
- Provider DTOs and wire names stay outside `shop-core` and `product-listing-core`.

### Search, watchlist, notification, and worker

- `src/search-filter-*` persists and projects saved filters, percolates current listing events, writes idempotent matches, and creates search-filter notifications.
- `src/watchlist-*` owns watchlist state, quotas, list reads, and recipient lookup; ProductListing event processing generates watchlist notifications.
- `src/notification-*` owns notification and delivery aggregates, PostgreSQL state, the EMAIL channel contract, and AWS SES/template adapter.
- `src/aura-historia-worker` routes committed CDC changes. ProductListing, search-filter, watchlist, and delivery scopes are implemented. The router emits `ShopOpenSearch` jobs for `shops`, but `WorkerScope` has no matching shop scope/consumer. This is a follow-on design blocker: decide and implement/retire the shop projection path together, never leave an unconsumed CDC route.

### Tests and docs

- Existing target tests span core behavior, service orchestration, PostgreSQL adapters, OpenSearch adapters, worker CDC/projection flows, crawler pipelines, Shopify Lambda, and API cases for shops, partner applications, listings, watchlist, search filters, and notifications.
- Future changes require focused tests at every changed boundary, plus migration/CDC stale-version/idempotency coverage where rows or projection inputs change.
- Contract sources are `docs/swagger.yaml`, `docs/product-listing.md`, `docs/events/flow.md`, `docs/storage.md`, `docs/arch.md`, and OpenSearch mappings. Update public docs only with shipped behavior.

## Dependency ledger

No crate gains or loses dependencies in iteration 0. The later design starts with no approved dependency changes; add or remove an edge only when its owning use case is approved and the direction remains `core <- service <- adapters <- runtime/transport`.

| Crate group | Expected gain | Expected loss |
| --- | --- | --- |
| `shop-core`, `shop-service`, `shop-postgres` | None | None |
| `shop-partner-core`, `shop-partner-service`, `shop-partner-postgres` | None | None |
| `product-listing-core`, `product-listing-service`, `product-listing-postgres`, `product-listing-opensearch`, `product-listing-translation-llm` | None | None |
| `aura-historia-api`, `aura-historia-worker`, `crawler`, `shopify-lambda` | None | None |
| `search-filter-core`, `search-filter-service`, `search-filter-postgres`, `search-filter-opensearch` | None | None |
| `watchlist-core`, `watchlist-service`, `watchlist-postgres` | None | None |
| `notification-core`, `notification-service`, `notification-postgres`, `notification-email`, `notification-email-aws` | None | None |
| `test-api` and target test suites | None | None |

## Follow-on blockers

1. Define the intended Shop aggregate shape and its compatibility/migration policy before modifying a canonical type, API, or table.
2. Decide whether the documented Shop OpenSearch route is implemented now or intentionally deferred; align worker scope, queue consumer, deployment, tests, mapping, and rebuild procedure in one change.
3. Classify any active `Product` wording touched by future work as Aura contract, provider/source boundary, human copy, fixture, or dated history before renaming it.
