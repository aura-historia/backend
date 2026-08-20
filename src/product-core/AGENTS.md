# DOX

## Purpose

- Own `product-core` crate.
- Own canonical Product domain types for migration.

## Core Design

- Domain-only crate.
- Root modules: `description`, `heuristics`, `product`, `product_event`, `product_id`, `product_image`, `product_lifecycle`, `product_search`, `product_slug_id`, `product_state`, `prohibited_content`, `sanitize`, `shops_product_id`, `sort_product_field`, `title`.
- `product::Product` is canonical aggregate. Fields private. Rehydrate boundary public for adapter crates.
- Product translations, embeddings, user-state read models, read joins, and FX snapshots stay outside this aggregate. `ProductPricing` stores source prices only; `ProductSaleValuation` records a sold-at timestamp plus immutable FX snapshot ID. Product user-state read models live in `product-service`.
- Uses `shop-core` identifiers plus `geo`, `money`, and `localization` values; `domain-primitives` stays for neutral event/outcome values.
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
