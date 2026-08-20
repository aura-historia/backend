# DOX

## Purpose

- Own `notification-email-aws` crate.
- Send EMAIL notification deliveries with S3 templates and SES v2.

## Core Design

- Implements service-owned `NotificationChannelSender` for EMAIL only.
- Resolves current `PRIMARY` email target through `EmailDeliveryTargetReader` after generic delivery claim.
- Owns email templates, localized subject/state text, S3 keys, SES mapping, and safe provider-error classification. Missing templates and invalid/configuration failures are permanent; timeouts, transport failures, throttling, and 5xx responses are retryable.
- Takes clients and typed config through constructor. No env read. No logs.

## Ownership

- This doc rules `src/notification-email-aws/**`.
- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo check -p notification-email-aws`
- `cargo test -p notification-email-aws --all-features`
