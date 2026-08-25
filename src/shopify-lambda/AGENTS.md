# DOX

## Purpose

- Own `shopify-lambda` crate.

## Core Design

- Worker Lambda for Shopify product ingestion from EventBridge through SQS.
- Root modules: `types`.
- Main neighbors: `application`, `platform-observability`, `platform-postgres`, `product-listing-service`, `product-listing-postgres`, `shop-core`, `shop-service`, `shop-postgres`, `fxrate-postgres`.
- Event/runtime edge crate. It parses provider-native SQS/EventBridge payloads and invokes canonical ProductListing service handlers; Postgres listing/event writes stay in ProductListing service.

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
