# DOX

## Purpose

- Own `search-filter-opensearch` crate.
- Own OpenSearch adapter for canonical search-filter index port.

## Core Design

- Writes canonical documents directly through `user_search_filters`.
- Builds percolator queries from complete canonical search-filter projection views using the public product percolator JSON builder.
- Uses Postgres `version` as OpenSearch external versioning; stale or duplicate writes are no-op outcomes.
- Persists every ProductSearch field and rejects incomplete or unknown persisted search payloads.
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
