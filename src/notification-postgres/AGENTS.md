# DOX

## Purpose

- Own canonical Notification PostgreSQL adapter.

## Core Design

- PostgreSQL owns notification and external-delivery state.
- Private rows and JSON payload mapping reconstruct typed Notification content. Product-listing V1 snapshots persist `listing_source_id`, `source_listing_id`, `listing_source_slug_id`, and `listing_source_name` with canonical types. Partnership application decisions use their own source column and immutable Party/ListingSource snapshot. V1 watchlist availability payloads use `AVAILABILITY_CHANGE` plus nullable `old_availability`/`new_availability` keys with exact canonical availability codes.
- Low-level notification and delivery-intent repositories share the caller transaction. The notification list reader joins the owner’s current `show_unassessed_or_sensitive_content` preference to the notification page, including empty pages. Generic delivery claim loads channel, target key, content, and current language/visibility preferences only. Watchlist price payloads serialize explicit source-currency prices. Channel-specific runtime adapters resolve targets after claim. Channel selection belongs to the notification-service planner, never this adapter.
- Implements the EMAIL target lookup contract with PostgreSQL. Worker only composes this adapter.
- Invalid persisted rows fail; never skip them.

## Verification

- `cargo check -p notification-postgres`
- `cargo test -p notification-postgres --all-features`
