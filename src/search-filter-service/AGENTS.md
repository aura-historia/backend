# DOX

## Purpose

- Own `search-filter-service` crate.
- Own search-filter use cases and outbound ports.

## Core Design

- Depends on `search-filter-core` and common ports only.
- Write use cases own transactions.
- Postgres and OpenSearch hidden behind ports.
- User list reads live in dedicated reader port, not repository.
- Search filter timestamps live on reader/index views, not aggregates.

## Ownership

- This doc rule `src/search-filter-service/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Verification

- `cargo check -p search-filter-service`
- `cargo test -p search-filter-service --all-features`
