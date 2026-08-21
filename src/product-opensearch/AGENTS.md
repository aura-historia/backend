# DOX

## Purpose

- Own `product-opensearch` crate.
- Own canonical Product OpenSearch adapters and private search documents.

## Core Design

- Depends on `product-core`, `product-service`, `shop-core`, `geo`, `money`/`localization` canonical values, and `platform-opensearch` generic response envelopes.
- Exports public OpenSearch reader factory/type, external-versioned Product projection writer, saved-filter percolator JSON builder, and typed-source-to-percolation JSON mapper.
- Keeps OpenSearch documents and mappings private; public percolation helpers expose no document type. Persistent Product pricing stays native or immutable sale-time: a sold no-main-price document has complete sale metadata but no `salePrices`; temporary percolation prices use the closed-world `priceByCurrency` shape.
- OpenSearch reads are ordinary readers. No transaction or unit of work.

## Ownership

- This doc rule `src/product-opensearch/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- Update this file when crate contract, dependency edge, index shape, or exported adapter changes.
- Keep `opensearch/mappings` aligned when document fields change.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Search documents do not escape this adapter.
- Map OpenSearch payloads into `product-service` read models.
- Preserve query-building tests for filters, cursors, canonical percolator semantics, pinned price conversion, and invalid sale documents.
- Price sorting is unsupported. Product search and similar readers consume one compiled request. Active summary prices use its pinned plan and sold summaries use exact indexed target values; summary valuation metadata names that current or sale basis. Product projection writes use `products.projection_version` with OpenSearch external versioning; conflicts are stale no-ops.

## Verification

- `cargo check -p product-opensearch`
- `cargo test -p product-opensearch --all-features`

## Child DOX Index

- None.
