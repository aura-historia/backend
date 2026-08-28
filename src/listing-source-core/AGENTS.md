# DOX

## Purpose

- Own ListingSource domain state and values.

## Core Design

- `ListingSource` owns stable ID/slug, name, Party operator, acquisition methods, presentation, and referral behavior. `outbound_url` is the canonical pure Partnerize-or-Aura-UTM URL derivation.
- Names trim Unicode outer whitespace, reject blank values, and allow at most 255 UTF-8 bytes without truncation. Creation derives an immutable slug once; empty slugification falls back to `listing-source-<listingSourceId>`.
- It has no provider secrets, lifecycle, search, address, or policy state.
- Rehydrate preserves valid stored slugs and rejects invalid persisted name or slug.

## Verification

- `cargo check -p listing-source-core`
- `cargo test -p listing-source-core --all-features`
