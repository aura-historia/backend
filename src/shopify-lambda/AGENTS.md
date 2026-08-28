# DOX

## Purpose

- Own `shopify-lambda` crate.

## Core Design

- Worker Lambda for Shopify product ingestion from EventBridge through SQS.
- Root modules: `types`.
- Main neighbors: `application`, `listing-source-core`, `listing-source-service`, `listing-source-postgres`, `platform-observability`, `platform-postgres`, `product-listing-service`, and `product-listing-postgres`.
- Event/runtime edge crate. It parses provider-native SQS/EventBridge payloads and invokes canonical ProductListing service handlers; Postgres listing/event writes stay in ProductListing service. Withdrawals resolve the configured Shopify ListingSource by domain and use `(ListingSourceId, SourceListingId)`.
- Shopify `active` create/update maps tracked positive inventory to `InStock`, tracked non-positive inventory to `OutOfStock`, and missing or untracked inventory to clear. `archived`, `draft`, and delete withdraw the matching listing; missing ListingSource or listing is acknowledged for idempotency. Missing or unsupported status is ignored without destructive write.

## Ownership

- This doc rule `src/shopify-lambda/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, route/event shape, env vars, or child index change.
- If trigger, retry, env var, queue/topic, or side effect change, update `infra/` and test wiring too.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Bootstrap thin. Push reusable work into service or domain crate.
- Be clear about event source, idempotency, and side effects.

## Verification

- `cargo check -p shopify-lambda`
- `cargo test -p shopify-lambda --all-features`

## Child DOX Index

- None.
