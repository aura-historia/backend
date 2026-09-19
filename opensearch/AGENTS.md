# DOX

## Purpose

- Own shared OpenSearch assets outside Rust crates.

## Core Design

- `analysis/` hold synonym lists. `mappings/` hold index mappings for ProductListings and user search filters.
- `hybrid-search-pipeline.json` holds current BM25+kNN RRF definition for explicit fresh setup via `deploy/bin/opensearch`. No rebuild/migration engine; existing mappings/analysis stay unchanged. Secured stock-node rehearsal covers these assets; Rust-client CA and maintained live engine remain separate gates.
- Runtime crates and infra depend on these files staying aligned with actual indexed documents. ProductListing mappings use `productListingTitleSlugId` (never `productListingSlugId`) and source identity only as `listingSourceId` and `sourceListingId` (never `sourceListingSlugId`); source presentation is hydrated from PostgreSQL. Saved-filter percolation targets use the repaired final field name `productListingTitleSlugId` only. Search-filter documents permit only `listingSourceId` and `excludeListingSourceId` source filters. No ListingSource index exists.

## Ownership

- This doc rule `opensearch/**`.
- Schema and analyzer drift hurt search hard. Treat change as contract change.
- Both mappings reserve boolean `projectionDeleted` for content-free durable projection fences. All owning-crate readers exclude true; absent remains live. Do not TTL or physically delete these documents: physical-delete version GC is not durable fencing. Expand mappings/readers before switching writers; retire old physical-delete writers. Owning adapter docs record rebuild limits and coordinator rollout needs. Both Faiss HNSW FP16 SQ encoders declare `bits: 16`, required by the selected OpenSearch 3.8 engine.

## Local Contracts

- Read root, then here, before edit.
- If mapping or analysis change, update code, infra, and docs that depend on it.
- Keep index names, field names, analyzers, and locale assets consistent.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Search contract first. Fancy later.

## Verification

- Read touched mapping or synonym file whole.
- Check matching Rust and infra references.

## Child DOX Index

- None.
