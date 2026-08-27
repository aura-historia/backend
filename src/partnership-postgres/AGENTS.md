# DOX

## Purpose

- Own PostgreSQL repositories/readers for Partnership and PartnershipApplication.

## Core Design

- `partnerships` is one Party-scoped aggregate table.
- `partnership_applications` persists validated proposal JSON, state codes, and optimistic version; submitted proposal data has no Party or ListingSource FK.
- Membership and ListingSource grants use idempotent join-table inserts.
- Rows/mappings remain private; public factories bind caller-owned SQLx transactions.

## Verification

- `cargo check -p partnership-postgres`
- `cargo test -p partnership-postgres --all-features`
