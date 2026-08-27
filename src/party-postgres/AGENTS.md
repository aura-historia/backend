# DOX

## Purpose

- Own `party-postgres` crate.
- Own Party SQLx adapters for PostgreSQL.

## Core Design

- Depends on `party-core`, `party-service`, and shared `platform-postgres` transaction mechanics.
- Exports only the public SQLx Party repository factory.
- Keeps SQL rows, SQL, mapping, and scoped repository private.
- Repository methods bind to caller-owned transactions, map persisted Party state with `TryFrom`, and enforce optimistic version updates.
- Integration tests use the business schema through `test-api`.

## Ownership

- This doc rules `src/party-postgres/**`.
- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo check -p party-postgres`
- `cargo test -p party-postgres --all-features`
- `cargo test -p party-postgres --tests`

## Child DOX Index

- None.
