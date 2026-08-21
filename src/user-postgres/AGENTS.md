# DOX

## Purpose

- Own `user-postgres` crate.
- Own canonical User SQLx adapters for Postgres.

## Core Design

- Depends on `user-core`, `user-service`, and shared `common` Postgres UoW primitives.
- Exports public SQLx factories only.
- Keeps SQL rows, SQL, mapping, repositories, and readers private.
- Readers and repositories bind to caller-owned transactions through service factory ports.
- `SqlxUserTierEntitlementsFactory` locks `users` with `FOR UPDATE`, then locks eligible watchlist rows before newest-first quota ranking in the same transaction. Changed watchlist rows increment their storage version.
- User repository writes use `RETURNING` and expose only storage-neutral persisted user state; delete returns row-existence only.
- `insert_if_absent` uses `ON CONFLICT (user_id) DO NOTHING` and returns the existing aggregate for idempotent `CreateUser` replay; email conflicts still fail.
- Access tokens stay outside this crate until their source-of-truth moves off DynamoDB.
- User search sort maps `Name` to `first_name`, then `last_name`; no score sort.

## Ownership

- This doc rule `src/user-postgres/**`.
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
- Integration tests mirror dedicated impl files one-to-one; duplicate helpers inline, no shared test support modules.

## Verification

- `cargo check -p user-postgres`
- `cargo test -p user-postgres --all-features`
- `cargo test -p user-postgres --tests` runs real Postgres integration tests split by implementation file.

## Child DOX Index

- None.
