# DOX

## Purpose

- Own `product-listing-service` crate.
- Own canonical ProductListing use-case contracts, handlers, and outbound ports.

## Core Design

- Depends on `product-listing-core`, `shop-core`/`shop-service` for Shopify intake eligibility, `notification-core`/`notification-service`, direct `application` contracts, and product-neutral `embedding`. Listing text search passes semantic query text to `EmbeddingGenerator::embed_search_query`; embedding failure falls back to BM25.
- Root modules: `ports`, `use_case_bundle`, `use_cases`.
- Write handlers use `application::transaction::UnitOfWork` and transaction-scoped ProductListing repository/event-store factories. This iteration retains existing `ProductState`, `ProductLifecycle`, and `ProductSaleValuation` behavior; the later availability and sale-observation rewrite owns those semantics.
- Partner ProductListing create, update, upsert, and delete use cases authorize admins or linked partner users inside their listing transaction. Partner-key writes use `(shopId, shopListingId)` aggregate lookup. Shopify and WooCommerce provider payloads remain provider-native at the boundary, then map into ProductListing commands and transact once.
- Repository writes return persisted ProductListing state; handlers must not read after write for responses.
- OpenSearch-backed ProductListing search captures a valuation instant and loads the required FX snapshot in a short repository read transaction. The handler then compiles one `ProductListingSearchReadRequest`; OpenSearch and embedding stay outside this transaction.
- `ProjectProductListing` rereads the committed current ProductListing source, validates the trigger event against its current event ID, then writes or deletes the rebuildable ProductListing OpenSearch projection with the authoritative monotonic projection version.
- `ProductListingUserStateReader` is an ordinary one-query batch read for relational state of OpenSearch result pages, including ordered unseen notification IDs. Its lookup contains only user and ProductListing IDs; adapters derive image safety from authoritative listing data. ProductListing details readers return the same complete `ProductListingUserState` in their caller transaction; no second notification read, per-listing read, or partial fallback.
- ProductListing detail, search, KNN, and watchlist result contracts use `application::personalized::Personalized<Item, ProductListingUserState>`; item views never inline `user_state`. `ProductListingDetailsReader` returns factual detail with source pricing and optional sale valuation.
- `EmbedProductListingEvent` rereads a committed current `DOMAIN_CREATED` ProductListing source, calls `EmbeddingGenerator::embed_product`, then atomically stores the vector, appends `ENRICHMENT_EMBEDDED`, and advances the ProductListing revision. Stale, duplicate, missing, ignored, and missing-title inputs are explicit outcomes.
- `GenerateWatchlistNotifications` owns ProductListing-event-driven watchlist notifications. It reads the immutable listing event/source, locks and rechecks the current listing revision, selects recipients by current watchlist intervals, then inserts in-app notifications and requested external delivery intents in one short PostgreSQL transaction.
- `ProductListingWatchlistDetailsReader` is a transaction-scoped, cursor-paged batch read contract for full localized personalized ProductListing views and relational user state. Its cursor uses watchlist creation time plus ProductListing ID.
- `ProductListingDetailsBatchReader` is an ordinary one-query batch read for full localized personalized ProductListing views and relational user state by ProductListing ID; callers preserve their own source order.
- `ProductListingSearchFilterMatchSourceReader` is a transaction-scoped canonical source for accepted current ProductListing CDC events. Its `find_sources` batch accepts exact `(ProductListingId, EventId)` refs and returns typed listing sources; adapter rows stay private. `ProductListingCurrentRevisionGuard` batch-locks and verifies authoritative ProductListing event IDs in the final match-write transaction. `ProductListingPercolationInput` carries only source plus closed-world converted values; no adapter receives an FX snapshot.
- Ports are public because adapter crates implement them.
- Port errors carry boxed sources for adapter/read-model failures; do not swallow underlying causes.
- No SQLx, OpenSearch, or transport dependency.

## Ownership

- This doc rule `src/product-listing-service/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, dependency edge, or use-case boundary changes.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Keep orchestration here. Keep rules in `product-listing-core`.
- Keep adapters outside.
- Keep unit tests inside the use-case file that owns the handler. No shared test-support module.

## Verification

- `cargo check -p product-listing-service`
- `cargo test -p product-listing-service --all-features`

## Child DOX Index

- None.
