# DOX

## Purpose

- Own `search-filter-postgres` crate.
- Own Postgres adapter for canonical search-filter ports.

## Core Design

- Implements `search-filter-service` repositories for `SqlxTransaction`.
- Implements ordinary `SqlxSearchFilterReader` for read models.
- Maps `search_filters` and `search_filter_matches` rows.
- Repository writes return storage-neutral persisted search-filter state.
- Product id is enough for product references.

## Ownership

- This doc rule `src/search-filter-postgres/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Verification

- `cargo check -p search-filter-postgres`
- `cargo test -p search-filter-postgres --all-features`
