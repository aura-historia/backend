# DOX

## Purpose

- Own `notification-aws` crate.
- Implement the canonical notification email-delivery sender with S3 templates and SES v2.

## Core Design

- Implements the service-owned `NotificationDeliverySender` port.
- Keeps AWS clients, S3 keys, SES request/tag mapping, and template rendering local. Recipient language and currency come from the delivery source; PostgreSQL defaults missing preferences to English/EUR.
- Takes clients and deployment/email config through its constructor; reads no environment variables and emits no logs.
- AWS SDK types stay inside this adapter. Only `SesNotificationDeliverySender` is public for runtime wiring.

## Ownership

- This doc rules `src/notification-aws/**`.
- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo test -p notification-aws --all-features`
- `cargo check --workspace`

## Child DOX Index

- None.
