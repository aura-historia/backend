# DOX

## Purpose

- Own ListingSource domain state and values.

## Core Design

- `ListingSource` owns stable ID/slug, name, Party operator, ingestion methods, presentation, and referral behavior. `outbound_url` is the canonical pure Partnerize-or-Aura-UTM URL derivation.
- Names trim Unicode outer whitespace, reject blank values, and allow at most 255 UTF-8 bytes without truncation. Partnerize `camref` is a preserved, nonblank ASCII alphanumeric path component (`[A-Za-z0-9]+`) of at most 128 bytes; it rejects trimming, delimiters, percent encoding, controls, and Unicode. Creation derives an immutable slug once; empty slugification falls back to `listing-source-<listingSourceId>`.
- It has no provider secrets, lifecycle, search, address, or policy state.
- Rehydrate preserves valid stored slugs and rejects invalid persisted name or slug.
- `lib.rs` exports the stable public API. Focused modules own the aggregate (`listing_source`), identifiers (`listing_source_id`, `listing_source_slug_id`), values (`listing_source_name`, `domain`, `listing_ingestion_method`), and referral behavior (`referral_configuration`).

## Verification

- `cargo check -p listing-source-core`
- `cargo test -p listing-source-core --all-features`
