# DOX

## Purpose

- Own `notification-core` crate.
- Own canonical notification domain and delivery reference types.

## Core Design

- Domain-only crate.
- Root modules: `notification`, `notification_delivery`, `notification_delivery_id`, `notification_kind`. `NotificationId` lives in `common::notification_id` so Product user state can use it without a dependency cycle.
- `notification_delivery` owns channel and opaque logical target-key values. EMAIL is the sole production channel; PUSH exists only to prove generic dispatch. New channel values belong here, not in producers or adapters.
- `Notification` aggregate has no created/updated, actor, delivery, or runtime metadata.
- Typed `NotificationContent` owns semantic source plus immutable display snapshot; kind is derived.
- Watchlist/search-filter product titles are optional.
- View types may carry created/updated timestamps.
- No DynamoDB, transport, or runtime glue.

## Ownership

- This doc rule `src/notification-core/**`.
- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo check -p notification-core`
- `cargo test -p notification-core --all-features`

## Child DOX Index

- None.
