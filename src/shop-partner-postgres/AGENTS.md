# DOX

## Purpose

- Own `shop-partner-postgres` crate.
- Own SQLx adapters for partner shop applications and partner shop list reads.

## Core Design

- Depends on `shop-partner-core`, `shop-partner-service`, `shop-service`, and shared Postgres UoW; it does not depend on the concrete `shop-postgres` adapter.
- Exports public SQLx factories only.
- Keeps SQL rows, mapping, repositories, and readers private.
- Partner-shop application repository writes use `RETURNING` and expose only storage-neutral persisted state.
- Owns the transaction-scoped, idempotent `user_partner_shops` membership writer used by approval.

## Ownership

- This doc rule `src/shop-partner-postgres/**`.
- Parent doc: `src/AGENTS.md`.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Preserve SQLx and mapping failures as boxed source errors.

## Verification

- `cargo check -p shop-partner-postgres`
- `cargo test -p shop-partner-postgres --all-features`

## Child DOX Index

- None.
