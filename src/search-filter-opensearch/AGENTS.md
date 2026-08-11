# DOX

## Purpose

- Own `search-filter-opensearch` crate.
- Own OpenSearch adapter for canonical search-filter index port.

## Core Design

- Writes canonical documents directly through `user_search_filters`.
- Builds percolator queries from complete canonical search-filter projection views using the public product percolator JSON builder, and implements `ProductMatchEvaluator` through direct Vertex AI Gemini access.
- Uses Postgres `version` as OpenSearch external versioning; stale or duplicate writes are no-op outcomes.
- Vertex matcher batches one product job: fetches at most five images once through `embedding::SafeImageFetcher`, evaluates enhanced filters with bounded request concurrency, uses 10s connect/30s total timeouts, structured JSON schema, and no product or image payload logging. Reasons are requested in the filter search language; product text falls back to English, then native text.
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
