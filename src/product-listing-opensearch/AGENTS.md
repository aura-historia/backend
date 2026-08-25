# DOX

## Purpose

- Own `product-listing-opensearch` crate.
- Own canonical Product Listing OpenSearch adapters and private search documents.

## Core Design

- Depends on `product-listing-core`, `product-listing-service`, `shop-core`, `geo`, `money`/`localization` canonical values, and `platform-opensearch` generic response envelopes.
- Exports public OpenSearch reader factory/type, external-versioned Product Listing projection writer, saved-filter percolator JSON builder, and typed-source-to-percolation JSON mapper.
- Keeps OpenSearch documents and mappings private; structural documents use canonical semantic leaves through local codecs, and public percolation helpers expose no document type. Product Listing language keeps the historical uppercase OpenSearch vocabulary through its local codec. Withdrawn listings delete their projection; active documents retain `ListingLifecycle::Active` and an optional `availability`. Persistent Product Listing pricing stays native unless an active `SoldOut` listing has an explicit sale observation: then `saleObservationFxRateId`, `saleObservedAt`, and optional `salePrices` use its immutable snapshot. Active relisted listings use current pricing; temporary percolation prices use the closed-world `priceByCurrency` shape.
- OpenSearch reads are ordinary readers. No transaction or unit of work.

## Ownership

- This doc rule `src/product-listing-opensearch/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- Update this file when crate contract, dependency edge, index shape, or exported adapter changes.
- Keep `opensearch/mappings` aligned when document fields change.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Search documents do not escape this adapter.
- Map OpenSearch payloads into `product-listing-service` read models.
- Preserve query-building tests for filters, cursors, canonical percolator semantics, pinned price conversion, and invalid sale-observation documents.
- Price sorting is unsupported. Product Listing search and similar readers consume one compiled request. Search filters use exact optional-availability fields and never lifecycle clauses. Active summary prices use its pinned plan and sold summaries use exact indexed target values; summary valuation metadata names the current or sale-observation basis. Product Listing projection writes use `product_listings.projection_version` with OpenSearch external versioning; conflicts are stale no-ops.

## Verification

- `cargo check -p product-listing-opensearch`
- `cargo test -p product-listing-opensearch --all-features`

## Child DOX Index

- None.
