# DOX

## Purpose

- Own `search-filter-service` crate.
- Own search-filter use cases and outbound ports.

## Core Design

- Depends on `search-filter-core`, common, public ProductSearch field types from `geo`, `isocountry`, and `shop-core`, plus canonical `user-service` tier-entitlements contracts.
- Write use cases own transactions.
- Postgres and OpenSearch hidden behind ports. Create/update build typed embedding text and call `embedding::TextEmbeddingGenerator` directly.
- Repository writes return persisted search-filter state.
- User list reads live in dedicated reader port, not repository.
- Create and update lock the authoritative user tier through transaction-scoped `UserTierEntitlements` before tier checks, active-filter counts, and writes; reactivation rechecks the stored full search and active-filter quota.
- Update generates an external embedding before the short write transaction, then revalidates the derived search state before persisting.
- Search filter timestamps live on reader/index views, not aggregates.
- CDC projection handlers reread complete Postgres index state then write through a versioned index port.
- Persisted-match lists compose one tie-safe match page, one batched Product-details read, and one batched notification read in the service. Returned Product order follows the match page.

## Ownership

- This doc rule `src/search-filter-service/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Verification

- `cargo check -p search-filter-service`
- `cargo test -p search-filter-service --all-features`
