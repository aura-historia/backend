# DOX

## Purpose

- Own `product-postgres` crate.
- Own canonical Product SQLx adapters for Postgres.

## Core Design

- Depends on `product-core`, `product-service`, `shop-core`, `notification-core`, `domain-primitives` versioning, `money`/`localization` canonical values, and shared `platform-postgres` UoW primitives.
- Exports public SQLx repository, event-store, factual product-details, product-history, product-embedding, Product user-state, batch product-details, batch watchlist-details, search-filter match source reader, and current-revision guard factories only. Factual detail, batch-detail, watchlist-detail, and match-source readers return source pricing plus optional immutable sale valuation; service owns exact-FX lookup and final pricing presentation. The match-source reader exposes immutable `product_events.event_time`, source event kind, and current Product event ID for stale-safe percolation. The current-revision guard holds `products` `FOR SHARE` through final match commit. Embedding source reader and writer reread committed current Product state, then lock/revalidate the source revision and atomically store vectors plus `ENRICHMENT_EMBEDDED` provenance events.
- The ordinary Product user-state reader resolves an OpenSearch result page in one set-based query: profile consent/tier, watchlist, selected search-filter match, Free-tier monthly hide state, and all unseen notification IDs ordered newest-first. Factual detail, batch-detail, and watchlist-detail readers return the same complete user state from their own SQL query.
- Keeps SQL rows, SQL, mappings, repositories, event stores, and reader internals private.
- Product row and `product_events` append bind to caller-owned transactions through service factory ports. The Product event-history reader returns domain events only.
- Product repository writes return storage-neutral persisted product state. Product source price columns contain no FX ID; `sale_fx_rate_id` plus `sold_at` persist the immutable sale valuation and are allowed only for `SOLD` or `REMOVED` state. Canonical FX storage and transactional latest-snapshot reads are owned by `fxrate-postgres`.
- Batch watchlist details use a tie-safe `created DESC, product_id ASC` cursor page with one joined query.
- Real Postgres integration tests live under `tests/` by implementation file, with helpers inline per file.

## Ownership

- This doc rule `src/product-postgres/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- Update this file when crate contract, dependency edge, SQL shape, or factory exports change.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Keep adapter types private unless composition root needs factories.
- Map rows with `TryFrom`; never leak SQLx row types.
- Preserve SQLx and row-mapping failures as error sources in service port errors.

## Verification

- `cargo check -p product-postgres`
- `cargo test -p product-postgres --all-features`
- `cargo test -p product-postgres --tests` runs real Postgres integration tests split by implementation file.

## Child DOX Index

- None.
