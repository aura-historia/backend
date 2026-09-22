# DOX

## Purpose

- Own shared concrete PostgreSQL and SQLx mechanics.

## Core Design

- Own typed pool config, neutral versioned credential/provider contracts, pool construction, and SQLx transaction implementation.
- Lambda profile keeps lazy min-zero pools, verified full TLS with configured CA path, bounded waits, stale-idle checks, and finite reuse. Migration profile separately requires caller-supplied `max_connections == 1`, verified full TLS, and longer bounded acquire/statement/lock/idle waits. Credential version leases and Secrets Manager access stay outside this crate. Direct profiled connections use the same bounded connect limit and startup timeout settings, not reusable session mutation. It emits safe structured acquisition and opened/reconnected-link metrics with no credentials or connection strings. Legacy `new` stays source-compatible for non-Lambda callers such as crawler.
- Depends on `application` transaction contracts and SQLx only.
- No entity repository, row, mapping, environment read, or business port.

## Ownership

- This doc rule `src/platform-postgres/**`.
- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo check -p platform-postgres --all-targets --all-features`
- `cargo test -p platform-postgres --all-features`
