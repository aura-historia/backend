# DOX

## Purpose

- Own `search-filter-service` crate.
- Own search-filter use cases and outbound ports.

## Core Design

- Depends on `search-filter-core`, common, public ProductSearch field types from `geo`, `isocountry`, and `shop-core`, plus canonical `user-service` account reader contracts.
- Write use cases own transactions.
- Postgres and OpenSearch hidden behind ports.
- Repository writes return persisted search-filter state.
- User list reads live in dedicated reader port, not repository.
- Create and update enforce tier quota and feature policy with canonical transaction-scoped user account reads; reactivation rechecks the stored full search and active-filter quota.
- Search filter timestamps live on reader/index views, not aggregates.
- Persisted-match lists compose one tie-safe match page, one batched Product-details read, and one batched notification read in the service. Returned Product order follows the match page.

## Ownership

- This doc rule `src/search-filter-service/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Verification

- `cargo check -p search-filter-service`
- `cargo test -p search-filter-service --all-features`
