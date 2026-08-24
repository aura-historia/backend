# DOX

## Purpose

- Own `notification-core` crate.
- Own canonical notification domain and delivery reference types.

## Core Design

- Domain-only crate.
- Root modules: `notification`, `notification_id`, `presentation`, `notification_delivery`, `notification_delivery_id`, `notification_kind`. `NotificationId` lives here. Product core has no dependency on notification core, so this keeps notification identity acyclic.
- `notification_delivery` owns channel and opaque logical target-key values. EMAIL is the sole channel. Future channel values belong here, with a matching schema migration, not in producers or adapters.
- `Notification` aggregate has no created/updated, actor, delivery, or runtime metadata.
- Typed `NotificationContent` owns semantic source plus immutable display snapshot; kind is derived. Watchlist price changes keep old/new prices as optional factual source-currency values; no FX conversion belongs here.
- Watchlist/search-filter product titles are optional.
- View types may carry created/updated timestamps.
- `presentation` owns the notification image presentation result and the centralized prohibited-content consent policy. It preserves the snapshot classification while optionally omitting the image URL.
- No transport or runtime glue.

## Ownership

- This doc rule `src/notification-core/**`.
- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo check -p notification-core`
- `cargo test -p notification-core --all-features`

## Child DOX Index

- None.
