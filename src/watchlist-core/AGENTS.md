# DOX

## Purpose

- Own `watchlist-core` crate.
- Own canonical ProductListing Watchlist domain types.

## Core Design

- Domain-only crate.
- Owns `WatchlistState`; its lifecycle is separate from Search Filter state.
- `WatchlistProductListing` uses canonical `user_id` and `ProductListingId` as identity.
- Aggregate has no persistence timestamps.
- No legacy, service, adapter, transport, or runtime dependency.

## Ownership

- This doc rule `src/watchlist-core/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Verification

- `cargo check -p watchlist-core`
- `cargo test -p watchlist-core --all-features`
