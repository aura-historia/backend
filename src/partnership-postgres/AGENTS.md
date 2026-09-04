# DOX

## Purpose

- Own PostgreSQL repositories/readers for Partnership and PartnershipApplication.

## Core Design

- `partnerships` is one Party-scoped aggregate table.
- `partnership_applications` persists validated proposal JSON, state codes, approval result IDs, and optimistic version; submitted proposal data has no Party or ListingSource FK.
- Membership and ListingSource grants use idempotent join-table inserts. A source grant belongs to `(partnership_id, listing_source_id)`; authorization requires both a user membership and that Partnership grant.
- Rows/mappings remain private; public factories bind caller-owned SQLx transactions.
- Admin application search uses one bounded keyset query with `(created|updated, partnership_application_id)` continuation, exact state/proposal/source filters, and inclusive timestamp ranges.

## Verification

- `cargo check -p partnership-postgres`
- `cargo test -p partnership-postgres --all-features`
