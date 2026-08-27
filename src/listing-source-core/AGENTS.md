# DOX

## Purpose

- Own ListingSource domain state and values.

## Core Design

- `ListingSource` owns stable ID/slug, name, Party operator, acquisition methods, presentation, and referral behavior.
- It has no provider secrets, lifecycle, search, address, or policy state.
- Rehydrate preserves any valid stored slug.

## Verification

- `cargo check -p listing-source-core`
- `cargo test -p listing-source-core --all-features`
