# DOX

## Purpose

- Own `product-core` crate.
- Own canonical Product domain types for migration.

## Core Design

- Domain-only crate.
- Root modules: `description`, `heuristics`, `product`, `product_event`, `product_image`, `product_search`, `prohibited_content`, `sanitize`, `sort_product_field`, `title`, `user_state`.
- `product::Product` is canonical aggregate. Fields private. Rehydrate boundary public for adapter crates.
- Product translations, embeddings, read joins, and FX snapshots stay outside this aggregate. `ProductPricing` stores source prices only; `ProductSaleValuation` records a sold-at timestamp plus immutable FX snapshot ID.
- Uses `money` and `localization` for canonical price, currency, language, and localized values; `domain-primitives` stays for neutral event/outcome values.
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
