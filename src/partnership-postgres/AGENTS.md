# DOX

## Purpose

- Own PostgreSQL repositories/readers for Partnership and PartnershipApplication.

## Core Design

- `partnerships` is one Party-scoped aggregate table.
- `partnership_applications` persists validated proposal JSON, state codes, approval result IDs, and optimistic version; submitted proposal data has no Party or ListingSource FK.
- Membership and ListingSource grants use idempotent join-table inserts. A source grant belongs to `(partnership_id, listing_source_id)`; authorization requires both a user membership and that Partnership grant.
- Rows/mappings remain private; public factories bind caller-owned SQLx transactions.
- Admin application search uses one bounded keyset query with `(created|updated, partnership_application_id)` continuation, exact state/proposal/source filters, and inclusive timestamp ranges.
- Admin partnership search uses one bounded joined reader query with `created DESC, partnership_id DESC` keyset pagination; filters use `EXISTS` so member and source-grant counts stay complete. The `partnerships_created_id_idx` index must match that order.

## Verification

- `cargo check -p partnership-postgres`
- `cargo test -p partnership-postgres --all-features`
