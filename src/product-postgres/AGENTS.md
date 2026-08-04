# DOX

## Purpose

- Own `product-postgres` crate.
- Own canonical Product SQLx adapters for Postgres.

## Core Design

- Depends on `product-core`, `product-service`, and shared `common` Postgres UoW primitives.
- Exports public SQLx repository, event-store, product-details, product-history, and product-embedding reader factories only.
- Keeps SQL rows, SQL, mappings, repositories, event stores, and reader internals private.
- Product row and `product_events` append bind to caller-owned transactions through service factory ports.
- Product repository writes return storage-neutral persisted product state.
- Real Postgres integration tests live under `tests/` by implementation file, with helpers inline per file.

## Ownership

- This doc rule `src/product-postgres/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- Update this file when crate contract, dependency edge, SQL shape, or factory exports change.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Keep adapter types private unless composition root needs factories.
- Map rows with `TryFrom`; never leak SQLx row types.
- Preserve SQLx and row-mapping failures as error sources in service port errors.

## Verification

- `cargo check -p product-postgres`
- `cargo test -p product-postgres --all-features`
- `cargo test -p product-postgres --tests` runs real Postgres integration tests split by implementation file.

## Child DOX Index

- None.
