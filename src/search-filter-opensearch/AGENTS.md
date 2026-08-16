# DOX

## Purpose

- Own `search-filter-opensearch` crate.
- Own OpenSearch adapter for canonical search-filter index port.

## Core Design

- Writes canonical documents directly through `user_search_filters`.
- Builds percolator queries from one complete service-compiled search-filter projection using the public product percolator JSON builder; it stores the compiled FX-rate ID. Percolation receives an optional exact sale snapshot from the service, never a latest FX snapshot.
- Uses Postgres `version` as OpenSearch external versioning; stale or duplicate writes are no-op outcomes.
- Vertex AI product matching is a service-orchestrated use of the neutral `large-language-model` capability, not part of this OpenSearch adapter.
- Persists every ProductSearch field and rejects incomplete or unknown persisted search payloads.
- Percolates complete deterministic result sets through a PIT with a bounded page size, stable `userSearchFilterId` sort, exact totals, and defensive ID deduplication; it fails instead of truncating or accepting partial results.
- Uses the shared application cursor default (currently 21) when a search-filter index query omits a cursor, never OpenSearch's implicit page size.
- Keeps OpenSearch document types private.

## Ownership

- This doc rule `src/search-filter-opensearch/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Work Guidance

- Product percolator JSON may cross from `product-opensearch`; product document types may not.
- Preserve complete ProductSearch round-trip and percolator tests.

## Verification

- `cargo check -p search-filter-opensearch`
- `cargo test -p search-filter-opensearch --all-features`
