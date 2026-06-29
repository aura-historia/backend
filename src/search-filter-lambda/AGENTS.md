## Purpose

- Own `search-filter-lambda` crate and child crate map.

## Ownership

- This doc rule `src/search-filter-lambda/**`.
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

- `cargo check -p search-filter-lambda`

## Child DOX Index

- `src/search-filter-lambda/src/search-filter-lambda-opensearch-sync/AGENTS.md` — `search-filter-lambda-opensearch-sync` crate.
- `src/search-filter-lambda/src/search-filter-lambda-percolate-product/AGENTS.md` — `search-filter-lambda-percolate-product` crate.
