# DOX

## Purpose

- Own `search-filter-postgres` crate.
- Own Postgres adapter for canonical search-filter ports.

## Core Design

- Implements `search-filter-service` repositories for `platform_postgres::SqlxTransaction` and maps persisted ProductSearch language, currency, monetary values, and Product lifecycle through private JSON storage types to canonical domain types.
- Implements ordinary `SqlxSearchFilterReader` for read models, `SqlxSearchFilterIndexReader` for complete versioned index reads, and focused transaction-scoped active-candidate, monthly notification-rank quota, match-write, and typed match-notification source reader factories.
- Maps `search_filters` and `search_filter_matches` rows, including nullable paired `CURRENT`/`EVENT`/`SALE` price-match FX provenance. Invalid partial or unknown persisted provenance fails mapping.
- Owns focused periodic-match candidate, existing-match, progress, and dedicated-session advisory-lock adapters. Candidates use the closed window end; progress SQL can only advance a checkpoint. The final progress lock holds the `search_filters` row and revalidates its `ACTIVE` state, selected version, and selected progress before match writes or checkpoint advancement. Periodic state remains separate from ordinary Search Filter views.
- Repository writes return storage-neutral persisted search-filter state.
- Product id is enough for product references; no `product-service` dependency.

## Ownership

- This doc rule `src/search-filter-postgres/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Verification

- `cargo check -p search-filter-postgres`
- `cargo test -p search-filter-postgres --all-features`
