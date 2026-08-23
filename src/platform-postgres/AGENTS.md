# DOX

## Purpose

- Own shared concrete PostgreSQL and SQLx mechanics.

## Core Design

- Own typed pool config, pool construction, and SQLx transaction implementation.
- Depends on `application` transaction contracts and SQLx only.
- No entity repository, row, mapping, environment read, or business port.

## Ownership

- This doc rule `src/platform-postgres/**`.
- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo check -p platform-postgres --all-targets --all-features`
- `cargo test -p platform-postgres --all-features`
