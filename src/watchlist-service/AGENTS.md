# DOX

## Purpose

- Own `watchlist-service` crate.
- Own watchlist use cases and outbound ports.
- Exposes watch, list, update, and unwatch product use cases.

## Core Design

- Depends on `watchlist-core`, Product and FX read contracts, Notification read contracts, and common ports.
- Write use cases own transactions.
- Persistence hidden behind repository factory.
- Repository writes return persisted watchlist state.
- List uses transaction-scoped Product watchlist-details and FX snapshot readers for one PostgreSQL cursor page. Service applies the Product pricing presentation policy: one latest FX snapshot for all current valuations and one batch lookup for sale valuation snapshots. Missing or invalid FX data fails explicitly. It commits before one PostgreSQL batch unseen-notification-ID read; notification failure fails the whole read.
- Watchlist pagination uses `created DESC, product_id ASC`; the cursor contains both values so tied creation times cannot skip or duplicate products.
- Product views are public `common::personalized::Personalized` Product-service contracts. Watchlist owns orchestration, authorization, notification hydration, and hidden-product redaction.
- Watchlist writes require `watchlist:write`.
- Create and reactivation lock the authoritative user tier through transaction-scoped `UserTierEntitlements` before quota counts and writes; quotas are Free 20, Pro 100, Ultimate unlimited.
- Watchlist list reads require owner/service/system access and delegated `watchlist:read`.

## Ownership

- This doc rule `src/watchlist-service/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Verification

- `cargo check -p watchlist-service`
- `cargo test -p watchlist-service --all-features`
