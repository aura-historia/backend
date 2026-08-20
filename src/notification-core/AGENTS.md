# DOX

## Purpose

- Own `notification-core` crate.
- Own canonical notification domain and mail template types.

## Core Design

- Domain-only crate.
- Root modules: `mail_template`, `notification`, `notification_id`, `notification_type`.
- `Notification` aggregate has no created/updated or actor metadata.
- Watchlist/search-filter product titles are optional.
- View types may carry created/updated timestamps.
- Uses canonical identifiers from `user-core`, `search-filter-core`, and `shop-partner-core`; uses `shop-core` shop values and pure `money`/`localization` values.
- No DynamoDB, transport, or runtime glue.

## Ownership

- This doc rule `src/notification-core/**`.
- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo check -p notification-core`
- `cargo test -p notification-core --all-features`

## Child DOX Index

- None.
