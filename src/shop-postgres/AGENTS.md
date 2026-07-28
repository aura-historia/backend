# DOX

## Purpose

- Own `shop-postgres` crate.
- Own canonical Shop SQLx adapters for Postgres.

## Core Design

- Depends on `shop-core`, `shop-service`, and shared `common` Postgres UoW primitives.
- Exports public SQLx factories only.
- Keeps SQL rows, SQL, mapping, repositories, and readers private.
- Readers and repositories bind to caller-owned transactions through service factory ports.
- Does not read or write `shops.view_url`; derive view URL from `url` and affiliate config.

## Ownership

- This doc rule `src/shop-postgres/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- Update this file when crate contract, dependency edge, SQL shape, or factory exports change.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Keep adapter types private.
- Map rows with `TryFrom`; never leak SQLx row types.
- Preserve SQLx and row-mapping failures as error sources in service port errors.

## Verification

- `cargo check -p shop-postgres`
- `cargo test -p shop-postgres --all-features`

## Child DOX Index

- None.
