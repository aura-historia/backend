# DOX

## Purpose

- Own PostgreSQL Auction repository, flat schedule columns, event journal, details reader, and batched safe summary reader.

## Core Design

- Rows and SQL stay private. Rehydration validates all persisted IDs, values, enum codes, localization pairs, and schedule bounds. Summary and directory hydration read flat schedule columns with each Auction root.
- Repository and journal writers bind to caller-owned `SqlxTransaction`. Auction updates use root CAS and write complete flat schedule state in that transaction.
- `auctions.listing_source_id` is restrictive. ListingSource deletion is blocked by retained Auctions.

## Verification

- `cargo check -p auction-postgres --all-targets --all-features`
- `cargo test -p auction-postgres --all-features`
