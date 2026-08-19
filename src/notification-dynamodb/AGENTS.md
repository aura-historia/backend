# DOX

## Purpose

- Own `notification-dynamodb` crate.
- Own DynamoDB notification adapter and records.

## Core Design

- Depends on `notification-core` and implements `notification-service` ports.
- Root modules: `currency_record`, `language_record`, `price_record`, `notification_record`, `notification_record_update`, `notification_reason_record`, `notification_type_record`, `repository`, `list_notifications_reader`, `all_notifications_reader`, `product_notifications_reader`, `batch_writer`, `deleter`.
- DynamoDB remains source of truth for notifications.
- `repository` owns aggregate persistence only: `insert`, `update`, `find_by_origin_event_id`.
- Repository writes return storage-neutral persisted notification state. The conditional writer returns `Inserted` only after DynamoDB persists the supplied notification and `AlreadyExists` on its conditional conflict; it never returns a fabricated retry notification.
- List/count/product reads live in dedicated one-file readers. Deletes and batch insert live in dedicated adapters.
- Records carry persistence timestamps. Records do not carry created_by/updated_by.
- Private DynamoDB currency, price, and language values map explicitly to `money` and `localization`; legacy serialized strings stay unchanged.

## Ownership

- This doc rule `src/notification-dynamodb/**`.
- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo check -p notification-dynamodb`
- `cargo test -p notification-dynamodb --all-features`

## Child DOX Index

- None.
