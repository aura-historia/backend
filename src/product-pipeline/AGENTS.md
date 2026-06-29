## Purpose

- Own `product-pipeline` crate and child crate map.

## Ownership

- This doc rule `src/product-pipeline/**`.
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

- `cargo check -p product-pipeline`

## Child DOX Index

- `src/product-pipeline/src/product-pipeline-embed-text/AGENTS.md` — `product-pipeline-embed-text` crate.
- `src/product-pipeline/src/product-pipeline-translate/AGENTS.md` — `product-pipeline-translate` crate.
