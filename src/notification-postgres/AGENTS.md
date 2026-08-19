# DOX

## Purpose

- Own canonical Notification PostgreSQL adapter.

## Core Design

- PostgreSQL owns notification and external-delivery state.
- Private rows and versioned JSON payload mapping reconstruct typed Notification content.
- Low-level notification and delivery-intent repositories share the caller transaction. Channel selection belongs to the notification-service planner, never this adapter.
- Invalid persisted rows fail; never skip them.

## Verification

- `cargo check -p notification-postgres`
- `cargo test -p notification-postgres --all-features`
