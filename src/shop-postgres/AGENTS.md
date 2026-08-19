# DOX

## Purpose

- Own `shop-postgres` crate.
- Own canonical Shop SQLx adapters for Postgres.

## Core Design

- Depends on `shop-core`, `shop-service`, pure `money`/`localization` values, and shared `platform-postgres` UoW primitives.
- Exports public SQLx factories only.
- Keeps SQL rows, SQL, mapping, repositories, and readers private.
- Shop repository writes use `RETURNING` and expose only storage-neutral persisted shop state.
- Readers and repositories bind to caller-owned transactions through service factory ports.
- Real Postgres integration tests live under `tests/` one file per dedicated adapter impl, with helpers inline per file.
- Does not read or write `shops.view_url`; derive view URL from `url` and affiliate config.
- Reads and writes `shops.lifecycle`; database default is `DRAFTED`. Public search and details readers return only `PUBLISHED` shops; admin workflows use repositories or dedicated admin readers. A dedicated transaction-scoped WooCommerce webhook reader exposes safe shop configuration; its paired verifier reads the secret, checks the HMAC, and returns only a semantic outcome. Rows and raw secrets stay private.

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
