# DOX

## Purpose

- Own `search-filter-opensearch` crate.
- Own OpenSearch adapter for canonical search-filter index port.

## Core Design

- Uses `user_search_filters` index.
- Builds percolator documents from canonical search filter reader views.
- Keeps OpenSearch document types private.

## Ownership

- This doc rule `src/search-filter-opensearch/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Verification

- `cargo check -p search-filter-opensearch`
- `cargo test -p search-filter-opensearch --all-features`
