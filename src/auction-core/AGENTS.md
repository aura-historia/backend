# DOX

## Purpose

- Own pure Auction aggregate, IDs, events, source key, name value, and exact-instant schedule values.

## Core Design

- `Auction` owns source-scoped immutable identity plus optional name, catalogue URL, format, schedule, and reported facts.
- Fields stay private. Creation and rehydration take supplied values; core creates no IDs, timestamps, or random values.
- Schedule fields are optional exact `OffsetDateTime` instants.
- No ProductListing, storage, API, resolver, or service dependency. Listing membership starts in later iteration 06.

## Ownership

- Parent rules: `../../AGENTS.md`, `../AGENTS.md`.
- `lib.rs` exports the deliberate cross-crate domain surface.

## Verification

- `cargo check -p auction-core --all-targets --all-features`
- `cargo test -p auction-core --all-features`
- `cargo clippy -p auction-core --all-targets --all-features -- -D warnings`
