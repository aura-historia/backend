# DOX

## Purpose

- Own shared OpenSearch assets outside Rust crates.

## Core Design

- `analysis/` hold synonym lists. `mappings/` hold index mappings for ProductListings and user search filters.
- Runtime crates and infra depend on these files staying aligned with actual indexed documents. ProductListing mappings use `productListingTitleSlugId` (never `productListingSlugId`) and source identity only as `listingSourceId` and `sourceListingId` (never `sourceListingSlugId`); source presentation is hydrated from PostgreSQL. Saved-filter percolation targets use the repaired final field name `productListingTitleSlugId` only. Search-filter documents permit only `listingSourceId` and `excludeListingSourceId` source filters. No ListingSource index exists.

## Ownership

- This doc rule `opensearch/**`.
- Schema and analyzer drift hurt search hard. Treat change as contract change.

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
