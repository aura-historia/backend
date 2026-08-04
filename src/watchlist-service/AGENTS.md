# DOX

## Purpose

- Own `watchlist-service` crate.
- Own watchlist use cases and outbound ports.
- Exposes watch, list, update, and unwatch product use cases.

## Core Design

- Depends on `watchlist-core`, Product read contracts, Notification read contracts, and common ports.
- Write use cases own transactions.
- Persistence hidden behind repository factory.
- Repository writes return persisted watchlist state.
- List uses a transaction-scoped Product watchlist-details reader for one joined PostgreSQL cursor page. It commits before one batch DynamoDB notification read; notification failure fails the whole read.
- Watchlist pagination uses `created DESC, product_id ASC`; the cursor contains both values so tied creation times cannot skip or duplicate products.
- Product views are public Product-service contracts. Watchlist owns orchestration, authorization, notification hydration, and hidden-product redaction.
- Watchlist writes require `watchlist:write`.
- Watchlist list reads require owner/service/system access and delegated `watchlist:read`.

## Ownership

- This doc rule `src/watchlist-service/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Verification

- `cargo check -p watchlist-service`
- `cargo test -p watchlist-service --all-features`
