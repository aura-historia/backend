# DOX

## Purpose

- Own ListingSource PostgreSQL repository and readers.

## Core Design

- Rows, SQL, provider configuration, and secrets stay adapter-private.
- Repository uses caller-owned `SqlxTransaction`; unknown persisted acquisition values fail.

## Verification

- `cargo check -p listing-source-postgres`
- `cargo test -p listing-source-postgres --all-features`
