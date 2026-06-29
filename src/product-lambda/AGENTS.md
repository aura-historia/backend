## Purpose

- Own `product-lambda` crate and child crate map.

## Ownership

- This doc rule `src/product-lambda/**`.
- Parent doc: `src/AGENTS.md`.
- Child docs below rule deeper child crates.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract or child index change.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Match crate pattern. Keep cross-crate bleed low.

## Verification

- `cargo check -p product-lambda`

## Child DOX Index

- `src/product-lambda/src/product-lambda-ingest-partner-products/AGENTS.md` — `product-lambda-ingest-partner-products` crate.
- `src/product-lambda/src/product-lambda-materialize-opensearch/AGENTS.md` — `product-lambda-materialize-opensearch` crate.
- `src/product-lambda/src/product-lambda-update-notify-user/AGENTS.md` — `product-lambda-update-notify-user` crate.
