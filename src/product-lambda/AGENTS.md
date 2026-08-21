# DOX

## Purpose

- Own `product-lambda` crate.

## Core Design

- Parent crate for async product workers.
- Child crates: `product-lambda-delete-product`, `product-lambda-ingest-partner-products`, `product-lambda-materialize-opensearch`.
- Main neighbors: `product-lambda-delete-product`, `product-lambda-ingest-partner-products`, `product-lambda-materialize-opensearch`.
- Parent crate exists to group child executables or suites and keep their map discoverable.

## Ownership

- This doc rule `src/product-lambda/**`.
- Parent doc: `src/AGENTS.md`.
- Child docs below rule deeper child crates.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, route/event shape, env vars, or child index change.
- Keep child crate list honest. Shared parent glue stay tiny.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Parent crate own map and shared glue. Real work live in child crates.

## Verification

- `cargo check -p product-lambda`

## Child DOX Index

- `src/product-lambda/src/product-lambda-delete-product/AGENTS.md` — `product-lambda-delete-product` crate.
- `src/product-lambda/src/product-lambda-ingest-partner-products/AGENTS.md` — `product-lambda-ingest-partner-products` crate.
- `src/product-lambda/src/product-lambda-materialize-opensearch/AGENTS.md` — `product-lambda-materialize-opensearch` crate.
