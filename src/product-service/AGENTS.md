# DOX

## Purpose

- Own `product-service` crate.
- Own canonical Product use-case contracts, handlers, and outbound ports for migration.

## Core Design

- Depends on `product-core`, `money`/`localization` canonical values, owning core IDs and values, `shop-core`/`shop-service` for Shopify intake eligibility, `notification-service`, shared `application` contracts, and product-neutral `embedding`. Product text search passes semantic query text to `EmbeddingGenerator::embed_search_query`; embedding failure falls back to BM25.
- Root modules: `ports`, `use_case_bundle`, `use_cases`, `user_state`. `user_state` owns Product presentation/read values outside the aggregate core.
- Write handlers use `application::transaction::UnitOfWork` and transaction-scoped repository/event-store factories. Sold Product creation and transitions capture `sold_at`, then rehydrate latest persisted FX snapshot at or before that instant through a transaction-scoped FX repository, then store immutable sale valuation; missing or invalid data rejects the write.
- Partner Product create, update, upsert, and delete use cases authorize admins or linked partner users inside their Product transaction. Partner-key writes use `(shopId, shopsProductId)` aggregate lookup. Shopify intake resolves published partner shops through `shop-service` then delegates the authoritative product/event transaction to Product upsert. WooCommerce intake is one Product use case: it uses direct transaction-scoped Shop membership/config/signature ports, maps provider payloads, writes canonical Product state/events, then commits once. It does not call Shop or Product use cases.
- Repository writes return persisted product state; handlers must not read after write for responses.
- OpenSearch-backed Product search first pages capture one valuation instant, then use a short repository read transaction to load latest persisted FX snapshot at or before it; cursor pages load the cursor snapshot ID. The handler commits, then compiles one `ProductSearchReadRequest` with scaled native ranges and pinned conversion data. Similar-Product reads also load one snapshot at or before their valuation instant before their OpenSearch KNN read. Active summaries use that snapshot; sold summaries use immutable indexed sale values. Missing, invalid, read, begin, and commit failures are explicit. OpenSearch and embedding stay outside this transaction.
- `ProjectProduct` rereads the committed current Product source, requires the trigger event to match its current event ID, loads an exact sale snapshot only when a sale valuation has a main source price to convert, commits, then writes or deletes the rebuildable Product OpenSearch projection with the authoritative monotonic projection version. A sold no-main-price projection preserves sale metadata without converted sale prices.
- `ProductUserStateReader` is an ordinary one-query batch read for relational state of OpenSearch result pages. Its lookup contains only the user and Product IDs; adapters derive image safety from authoritative Product data. Search and KNN handlers compose it with one required all-user DynamoDB notification read; no per-product reads or partial fallback.
- Product detail, search, KNN, and watchlist result contracts use `application::personalized::Personalized<Item, ProductUserState>`; item views never inline `user_state`. `ProductDetailsReader` returns factual detail with source pricing and optional sale valuation. `GetProduct` loads the sale snapshot by ID, or the latest current snapshot, then owns HalfUp currency presentation and its valuation metadata before commit.
- `EmbedProductEvent` rereads a committed current `DOMAIN_CREATED` Product source, supplies title, optional additional text, and optional image to `EmbeddingGenerator::embed_product` before a short Postgres transaction, then atomically stores the vector, appends `ENRICHMENT_EMBEDDED`, and advances the Product revision. Stale, duplicate, missing, ignored, and missing-title inputs are explicit outcomes.
- `GenerateWatchlistNotifications` owns Product-event-driven watchlist notifications: it reads the immutable Product event/source and active recipients in one short Postgres transaction, commits, then invokes the Notification write use case. Its result reports inserted and deduplicated counts separately. DynamoDB work stays outside the Postgres transaction.
- `ProductWatchlistDetailsReader` is a transaction-scoped, cursor-paged batch read contract for full localized personalized Product views and relational user state. Its cursor uses watchlist creation time plus Product ID.
- `ProductDetailsBatchReader` is an ordinary one-query batch read for full localized personalized Product views and relational user state by Product ID; callers preserve their own source order.
- `ProductSearchFilterMatchSourceReader` is a transaction-scoped canonical source for accepted current Product CDC events. It returns typed Product, Shop, localized text, image, native pricing, optional immutable `ProductSaleValuation`, immutable `product_events.event_time`, and query data; adapter rows stay private. `ProductCurrentRevisionGuard` locks and verifies the authoritative Product event ID in the final match-write transaction. `ProductPercolationInput` carries only source plus closed-world converted values; no adapter receives an FX snapshot.
- Ports are public because adapter crates implement them.
- Port errors carry boxed sources for adapter/read-model failures; do not swallow underlying causes.
- Legacy Shop and Notification contract values map only through the private `legacy_values` bridge; canonical Product types stay `money`/`localization`.
- No SQLx, DynamoDB, OpenSearch, transport, or legacy `product` dependency.

## Ownership

- This doc rule `src/product-service/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, dependency edge, or use-case boundary changes.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Keep orchestration here. Keep rules in `product-core`.
- Keep adapters outside.
- Keep unit tests inside the use-case file that owns the handler. No shared test-support module.

## Verification

- `cargo check -p product-service`
- `cargo test -p product-service --all-features`

## Child DOX Index

- None.
