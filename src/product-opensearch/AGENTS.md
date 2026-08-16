# DOX

## Purpose

- Own `product-opensearch` crate.
- Own canonical Product OpenSearch adapters and private search documents.

## Core Design

- Depends on `product-core`, `product-service`, `shop-core`, `geo`, and `common` OpenSearch response types.
- Exports public OpenSearch reader factory/type, compiled-Product-search percolator JSON builder, and typed-source-to-percolation JSON mapper.
- Keeps OpenSearch documents and mappings private; public percolation helpers expose no document type. Source prices stay native; active percolation receives no FX snapshot, while sale percolation renders every display Currency from the exact immutable sale snapshot using HalfUp.
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
- Price sorting is unsupported; readers consume one compiled request. Active prices use its pinned plan and sold prices use exact indexed target values.

## Verification

- `cargo check -p product-opensearch`
- `cargo test -p product-opensearch --all-features`

## Child DOX Index

- None.
