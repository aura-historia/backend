# DOX

## Purpose

- Own `product-service` crate.
- Own canonical Product use-case contracts, handlers, and outbound ports for migration.

## Core Design

- Depends on `product-core`, `notification-service`, shared `common` app contracts, and product-neutral `embedding`. Product text search passes semantic query text to `EmbeddingGenerator::embed_search_query`; embedding failure falls back to BM25.
- Root modules: `ports`, `use_case_bundle`, `use_cases`.
- Write handlers use `common::transaction::UnitOfWork` and transaction-scoped repository/event-store factories.
- Partner Product create, update, upsert, and delete use cases authorize admins or linked partner users inside their Product transaction. Partner-key writes use `(shopId, shopsProductId)` aggregate lookup.
- Repository writes return persisted product state; handlers must not read after write for responses.
- OpenSearch-backed search is an ordinary reader. Do not model it as transactional.
- `ProductUserStateReader` is an ordinary one-query batch read for relational state of OpenSearch result pages. Its lookup contains only the user and Product IDs; adapters derive image safety from authoritative Product data. Search and KNN handlers compose it with one required all-user DynamoDB notification read; no per-product reads or partial fallback.
- Product detail, search, KNN, and watchlist result contracts use `common::personalized::Personalized<Item, ProductUserState>`; item views never inline `user_state`.
- `EmbedProductEvent` rereads a committed current `DOMAIN_CREATED` Product source, supplies title, optional additional text, and optional image to `EmbeddingGenerator::embed_product` before a short Postgres transaction, then atomically stores the vector, appends `ENRICHMENT_EMBEDDED`, and advances the Product revision. Stale, duplicate, missing, ignored, and missing-title inputs are explicit outcomes.
- `GenerateWatchlistNotifications` owns Product-event-driven watchlist notifications: it reads the immutable Product event/source and active recipients in one short Postgres transaction, commits, then invokes the Notification write use case. Its result reports inserted and deduplicated counts separately. DynamoDB work stays outside the Postgres transaction.
- `ProductWatchlistDetailsReader` is a transaction-scoped, cursor-paged batch read contract for full localized personalized Product views and relational user state. Its cursor uses watchlist creation time plus Product ID.
- `ProductDetailsBatchReader` is an ordinary one-query batch read for full localized personalized Product views and relational user state by Product ID; callers preserve their own source order.
- `ProductSearchFilterMatchSourceReader` is a transaction-scoped canonical current-Product source for accepted Product CDC events. It returns typed Product, Shop, localized text, image, pricing, and query data; adapter rows stay private.
- Ports are public because adapter crates implement them.
- Port errors carry boxed sources for adapter/read-model failures; do not swallow underlying causes.
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
