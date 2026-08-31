# DOX

## Purpose

- Own `notification-email-aws` crate.
- Send EMAIL notification deliveries with S3 templates and SES v2.

## Core Design

- Implements service-owned `NotificationChannelSender` for EMAIL only.
- Consumes the EMAIL target-reader contract from `notification-email`. Channel-specific runtime wiring resolves the current `PRIMARY` target after generic delivery claim.
- Owns email templates, localized subject/availability text, S3 keys, SES mapping, and safe provider-error classification. Availability template data uses nullable `old_availability`/`new_availability` values. Template data consumes the service-owned already-presented image URL; it does not decide listing content visibility. Watchlist price data remains in its immutable source currency; no FX conversion is applied. Missing templates and invalid/configuration failures are permanent; timeouts, transport failures, throttling, and 5xx responses are retryable.
- Partnership application approval/rejection templates use immutable `party_name`, `listing_source_name`, and optional `image_url` snapshot fields. Takes clients and typed config through constructor. No env read. No logs.
- Adapter-local template contract test strictly renders all 15 localized ProductListing MJML sources: five SearchFilter matches, five Watchlist availability changes, and five Watchlist price changes. It verifies the delivery data fields used by each template; deploy CI compiles MJML to S3 HTML.

## Ownership

- This doc rules `src/notification-email-aws/**`.
- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo check -p notification-email-aws`
- `cargo test -p notification-email-aws --all-features`
