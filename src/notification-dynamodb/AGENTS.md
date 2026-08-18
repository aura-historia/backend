# DOX

## Purpose

- Own `notification-dynamodb` crate.
- Own legacy DynamoDB notification adapter and records retained only for untouched periodic matcher compilation.

## Core Design

- Depends on `notification-core` and implements `notification-service` ports.
- Root modules: `notification_record`, `notification_record_update`, `notification_reason_record`, `notification_type_record`, `repository`, `list_notifications_reader`, `all_notifications_reader`, `product_notifications_reader`, `batch_writer`, `deleter`.
- PostgreSQL is notification truth; this crate has no production notification path.
- `repository` owns aggregate persistence only: `insert`, `update`, `find_by_origin_event_id`.
- Repository writes return storage-neutral persisted notification state. The conditional writer returns `Inserted` only after DynamoDB persists the supplied notification and `AlreadyExists` on its conditional conflict; it never returns a fabricated retry notification.
- List/count/product reads live in dedicated one-file readers. Deletes and batch insert live in dedicated adapters.
- Records carry persistence timestamps. Records do not carry created_by/updated_by.

## Ownership

- This doc rule `src/notification-dynamodb/**`.
- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo check -p notification-dynamodb`
- `cargo test -p notification-dynamodb --all-features`

## Child DOX Index

- None.
