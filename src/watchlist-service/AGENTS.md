# DOX

## Purpose

- Own `watchlist-service` crate.
- Own watchlist use cases and outbound ports.
- Exposes watch, list, update, and delete watchlist product use cases.

## Core Design

- Depends on `watchlist-core` and common ports only.
- Write use cases own transactions.
- Persistence hidden behind repository factory.
- Repository writes return persisted watchlist state.
- User/product list reads live in dedicated transaction-scoped reader port/factory, not repository.
- Watchlist writes require `watchlist:write`.
- Watchlist list reads require owner/service/system access and delegated `watchlist:write` until `WatchlistRead` exists in `common`.

## Ownership

- This doc rule `src/watchlist-service/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Verification

- `cargo check -p watchlist-service`
- `cargo test -p watchlist-service --all-features`
