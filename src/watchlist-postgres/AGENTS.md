# DOX

## Purpose

- Own `watchlist-postgres` crate.
- Own Postgres adapter for canonical watchlist ports.

## Core Design

- Implements `watchlist-service` repositories for `SqlxTransaction`.
- Implements transaction-scoped `SqlxWatchlistReaderFactory` for read models and `SqlxWatchlistQuotaReaderFactory` for tier-policy invariants.
- Maps `product_watchlist` rows to `watchlist-core` domain or reader views.
- Repository writes return storage-neutral persisted watchlist state. Insert and update maintain `active_since` and `notifications_enabled_since` as current-interval metadata.
- Notification recipient reads require active interval start at or before persisted Product event time and suppress email when the current email interval started later.
- Schema key is `(user_id, product_id)`.

## Ownership

- This doc rule `src/watchlist-postgres/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Verification

- `cargo check -p watchlist-postgres`
- `cargo test -p watchlist-postgres --all-features`
