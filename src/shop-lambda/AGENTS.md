# DOX

## Purpose

- Own `shop-lambda` crate.

## Core Design

- Parent crate for async shop workers.
- Child crates: `shop-lambda-opensearch-index`.
- Main neighbors: `shop-lambda-opensearch-index`.
- Parent crate exists to group child executables or suites and keep their map discoverable.

## Ownership

- This doc rule `src/shop-lambda/**`.
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

- `cargo check -p shop-lambda`

## Child DOX Index

- `src/shop-lambda/src/shop-lambda-opensearch-index/AGENTS.md` — `shop-lambda-opensearch-index` crate.
