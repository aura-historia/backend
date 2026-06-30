# DOX

## Purpose

- Own `search-filter-lambda` crate.

## Core Design

- Parent crate for async search filter workers.
- Child crates: `search-filter-lambda-opensearch-sync`, `search-filter-lambda-percolate-product`.
- Main neighbors: `search-filter-lambda-opensearch-sync`, `search-filter-lambda-percolate-product`.
- Parent crate exists to group child executables or suites and keep their map discoverable.

## Ownership

- This doc rule `src/search-filter-lambda/**`.
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

- `cargo check -p search-filter-lambda`

## Child DOX Index

- `src/search-filter-lambda/src/search-filter-lambda-opensearch-sync/AGENTS.md` — `search-filter-lambda-opensearch-sync` crate.
- `src/search-filter-lambda/src/search-filter-lambda-percolate-product/AGENTS.md` — `search-filter-lambda-percolate-product` crate.
