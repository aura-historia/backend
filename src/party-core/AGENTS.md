# DOX

## Purpose

- Own `party-core` crate.
- Own canonical Party domain types.

## Core Design

- Domain-only crate.
- Root modules: `party`, `party_id`, `party_name`, `party_slug_id`.
- `party::Party` has private identity, stable slug, name, and contact fields. Creation derives its slug once; rename preserves it. Rehydration accepts a valid persisted slug without comparing it to name.
- No events, role/type, lifecycle, merge, or address behavior.
- No dependency on `party-service` or adapters.

## Ownership

- This doc rules `src/party-core/**`.
- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo check -p party-core`
- `cargo test -p party-core --all-features`

## Child DOX Index

- None.
