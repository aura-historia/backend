# DOX

## Purpose

- Own canonical Notification PostgreSQL adapter.

## Core Design

- PostgreSQL owns notification and email-delivery state.
- Private rows and versioned JSON payload mapping reconstruct typed Notification content.
- Creation shares caller transaction and inserts requested email deliveries atomically.
- Invalid persisted rows fail; never skip them.

## Verification

- `cargo check -p notification-postgres`
- `cargo test -p notification-postgres --all-features`
