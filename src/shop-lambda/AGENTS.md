## Purpose

- Own `shop-lambda` crate and child crate map.

## Ownership

- This doc rule `src/shop-lambda/**`.
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

- `cargo check -p shop-lambda`

## Child DOX Index

- `src/shop-lambda/src/shop-lambda-opensearch-index/AGENTS.md` — `shop-lambda-opensearch-index` crate.
