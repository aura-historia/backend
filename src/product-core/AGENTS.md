# DOX

## Purpose

- Own `product-core` crate.
- Own canonical Product domain types for migration.

## Core Design

- Domain-only crate.
- Root modules: `description`, `fx_rate_id`, `fx_rate_snapshot`, `heuristics`, `product`, `product_event`, `product_image`, `product_search`, `prohibited_content`, `sanitize`, `sort_product_field`, `title`, `user_state`.
- `product::Product` is canonical aggregate. Fields private. Rehydrate boundary public for adapter crates.
- Product translations, embeddings, and read joins stay outside aggregate. Immutable EUR FX snapshots validate complete positive scaled-`Rate` quotes here.
- No dependency on `product-service`, legacy `product`, or adapters.

## Ownership

- This doc rule `src/product-core/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract or dependency edge changes.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Keep business rules here.
- No persistence, transport, or runtime glue.

## Verification

- `cargo check -p product-core`
- `cargo test -p product-core --all-features`

## Child DOX Index

- None.
