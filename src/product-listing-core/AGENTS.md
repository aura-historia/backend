# DOX

## Purpose

- Own `product-listing-core` crate.
- Own canonical ProductListing domain types.

## Core Design

- Domain-only crate.
- Root modules: `description`, `heuristics`, `listing_availability`, `listing_lifecycle`, `listing_orderability`, `product_listing`, `product_listing_id`, `product_listing_image`, `product_listing_search`, `product_listing_slug_id`, `prohibited_content`, `sanitize`, `shop_listing_id`, `sort_product_listing_field`, `title`.
- `product_listing::ProductListing` is canonical aggregate. Fields private. Rehydrate boundary public for adapter crates.
- ProductListing translations, embeddings, user-state read models, read joins, and FX snapshots stay outside this aggregate. `ProductListingPricing` stores source prices only. `ListingSaleObservation` retains an explicit observed-at timestamp plus immutable FX snapshot ID; it is separate from availability and lifecycle. `ProductListingPriceValuationBasis` persists canonical `CURRENT`, `EVENT`, and `SALE_OBSERVATION` values. ProductListing user-state read models live in `product-listing-service`.
- `ProductListingKey` owns only the semantic `(ShopId, ShopListingId)` pair; labeled storage and transport codecs live at their owning boundaries.
- `EnhancedSearchDescription` canonicalizes outer Unicode whitespace, rejects blank values, and caps stored text at 1000 bytes; raw-text construction is fallible.
- Uses `shop-core` identifiers plus `geo`, `money`, and `localization` values; `domain-primitives` stays for neutral event/outcome values.
- No dependency on `product-listing-service` or adapters.

## Ownership

- This doc rule `src/product-listing-core/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, persisted value, or dependency edge changes.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Keep business rules here.
- No persistence, transport, or runtime glue.

## Verification

- `cargo check -p product-listing-core`
- `cargo test -p product-listing-core --all-features`

## Child DOX Index

- None.
