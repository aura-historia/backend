# DOX

## Purpose

- Own `product` crate.

## Core Design

- Product domain, repositories, and core product services.
- Root modules: `core`, `data`, `dynamodb`, `opensearch`, `postgres`, `service`.

- Main neighbors: `common`, `fxrate`, `geo`, `shop`.
- Library crate. Keep domain, persistence, and service seams explicit.

## Ownership

- This doc rule `src/product/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, route/event shape, env vars, or child index change.
- Keep business rules here, not leaked into callers.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Exactly one repository per data source: DynamoDB, OpenSearch, Postgres.
- Product service facade hides source/storage choice from callers and uses domain types.
- Keep transport and runtime glue out of domain core.
- Postgres module owns SQL for products and product-events. Persist product row and product-events in one transaction.
- Postgres must not map through DynamoDB records.

## Verification

- `cargo check -p product`
- `cargo test -p product --all-features`

## Child DOX Index

- None.
