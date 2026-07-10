# DOX

## Purpose

- Own `product-api-partner` crate.

## Core Design

- Partner-facing product API Lambda for ingest, update, and delete flows.
- Root modules: `delete_product`, `patch_products`, `post_products`, `put_products`.
- Library crate. Keep domain, persistence, and service seams explicit.

## Ownership

- This doc rule `src/product-api-partner/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, route/event shape, env vars, or child index change.
- Keep business rules here, not leaked into callers.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Service and repository split stay clean.
- Keep transport and runtime glue out of domain core.

## Verification

- `cargo check -p product-api-partner`
- `cargo test -p product-api-partner --all-features`

## Child DOX Index

- None.
