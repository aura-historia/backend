# DOX

## Purpose

- Own `product-pipeline` crate.

## Core Design

- Parent crate for product enrichment workers.
- Child crate: `product-pipeline-embed-text`.
- Main neighbor: `product-pipeline-embed-text`. Product translation migrated to `aura-historia-worker`.
- Parent crate exists to group child executables or suites and keep their map discoverable.

## Ownership

- This doc rule `src/product-pipeline/**`.
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

- `cargo check -p product-pipeline`

## Child DOX Index

- `src/product-pipeline/src/product-pipeline-embed-text/AGENTS.md` — `product-pipeline-embed-text` crate.
