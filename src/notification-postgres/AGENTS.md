# DOX

## Purpose

- Own canonical Notification PostgreSQL adapter.

## Core Design

- PostgreSQL owns notification and external-delivery state.
- Private rows and JSON payload mapping reconstruct typed Notification content. V1 watchlist availability payloads use `AVAILABILITY_CHANGE` plus nullable `old_availability`/`new_availability` keys with exact canonical availability codes.
- Low-level notification and delivery-intent repositories share the caller transaction. The notification list reader joins the owner’s current prohibited-content consent to the notification page, including empty pages. Generic delivery claim loads channel, target key, content, and current language/consent preferences only. Watchlist price payloads serialize explicit source-currency prices. Channel-specific runtime adapters resolve targets after claim. Channel selection belongs to the notification-service planner, never this adapter.
- Implements the EMAIL target lookup contract with PostgreSQL. Worker only composes this adapter.
- Invalid persisted rows fail; never skip them.

## Verification

- `cargo check -p notification-postgres`
- `cargo test -p notification-postgres --all-features`
