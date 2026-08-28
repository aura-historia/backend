# DOX

## Purpose

- Own ListingSource PostgreSQL repository and readers.

## Core Design

- Rows, SQL, provider configuration, and secrets stay adapter-private.
- Repository uses caller-owned `SqlxTransaction`; unknown persisted acquisition values fail.
- `lib.rs` only declares and re-exports; the aggregate repository lives in `repositories/listing_source_repository.rs`.
- Each reader implementation owns one `readers/<capability>.rs` file; `readers/mod.rs` holds only shared adapter state and helpers.

## Verification

- `cargo check -p listing-source-postgres`
- `cargo test -p listing-source-postgres --all-features`
