# DOX

## Purpose

- Own canonical OAuth PostgreSQL adapter.

## Core Design

- Own SQLx rows, mappings, PostgreSQL repositories, purpose-specific pool readers, and transaction factories.
- Client aggregate writes use optimistic `version`; PostgreSQL `created`/`updated` metadata is returned through storage-neutral persisted views for write responses and reader views. Client secrets persist as hashes; only the narrow OAuth client-authentication reader loads hash material for service-side verification. Exchange-code bearer tokens stay adapter-private and never log.
- Authorization and third-party codes consume with one `DELETE ... RETURNING` query.

## Ownership

- This doc rule `src/oauth-postgres/**`.
- Parent doc: `src/AGENTS.md`.

## Local Contracts

- Read repo root, `src/AGENTS.md`, then here before edit.
- Update this doc when schema, port, or factory contract changes.

## Verification

- `cargo check -p oauth-postgres`
- `cargo test -p oauth-postgres --all-features`

## Child DOX Index

- None.
