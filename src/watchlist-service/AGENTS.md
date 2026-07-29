# DOX

## Purpose

- Own `watchlist-service` crate.
- Own watchlist use cases and outbound ports.

## Core Design

- Depends on `watchlist-core` and common ports only.
- Write use cases own transactions.
- Persistence hidden behind repository factory.
- User/product list reads live in dedicated transaction-scoped reader port/factory, not repository.

## Ownership

- This doc rule `src/watchlist-service/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Verification

- `cargo check -p watchlist-service`
- `cargo test -p watchlist-service --all-features`
