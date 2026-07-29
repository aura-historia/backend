# DOX

## Purpose

- Own `product-postgres` crate.
- Own canonical Product SQLx adapters and Postgres-backed handlers.

## Core Design

- Depends on `product-core`, `product-service`, and shared `common` Postgres UoW primitives.
- Exports public SQLx repository/event-store factories and Postgres handler types.
- Keeps SQL rows, SQL, mapping, repositories, and event-store internals private.
- Product row and `product_events` append stay in one caller-owned transaction.

## Ownership

- This doc rule `src/product-postgres/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- Update this file when crate contract, dependency edge, SQL shape, or factory exports change.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Keep adapter types private unless composition root needs them.
- Map rows with `TryFrom`; never leak SQLx row types.
- Preserve SQLx and row-mapping failures as error sources in service port errors.

## Verification

- `cargo check -p product-postgres`
- `cargo test -p product-postgres --all-features`

## Child DOX Index

- None.
