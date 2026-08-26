# DOX

## Purpose

- Own `notification-email-aws` crate.
- Send EMAIL notification deliveries with S3 templates and SES v2.

## Core Design

- Implements service-owned `NotificationChannelSender` for EMAIL only.
- Consumes the EMAIL target-reader contract from `notification-email`. Channel-specific runtime wiring resolves the current `PRIMARY` target after generic delivery claim.
- Owns email templates, localized subject/availability text, S3 keys, SES mapping, and safe provider-error classification. Availability template data uses nullable `old_availability`/`new_availability` values. Template data consumes service-owned language/consent preferences and the notification-core image policy, preserving prohibited-content classification while omitting unsafe image URLs without consent. Watchlist price data remains in its immutable source currency; no FX conversion is applied. Missing templates and invalid/configuration failures are permanent; timeouts, transport failures, throttling, and 5xx responses are retryable.
- Takes clients and typed config through constructor. No env read. No logs.

## Ownership

- This doc rules `src/notification-email-aws/**`.
- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo check -p notification-email-aws`
- `cargo test -p notification-email-aws --all-features`
