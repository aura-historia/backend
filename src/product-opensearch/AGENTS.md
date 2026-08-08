# DOX

## Purpose

- Own `product-opensearch` crate.
- Own canonical Product OpenSearch adapters and private search documents.

## Core Design

- Depends on `product-core`, `product-service`, `shop-core`, `geo`, and `common` OpenSearch response types.
- Exports public OpenSearch reader factory/type and JSON-only canonical percolator builder.
- Keeps OpenSearch documents and mappings private; the percolator builder exposes no document type.
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
- Preserve query-building tests for every filter/sort/cursor branch and canonical percolator semantics.

## Verification

- `cargo check -p product-opensearch`
- `cargo test -p product-opensearch --all-features`

## Child DOX Index

- None.
