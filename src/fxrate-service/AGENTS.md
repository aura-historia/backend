# DOX

## Purpose

- Own `fxrate-service` capture use case and FX capability ports.

## Core Design

- Capture uses canonical `money::Currency` quotes before a short PostgreSQL transaction.
- Write port inserts one immutable snapshot idempotently by source event ID.
- Repository rehydrates immutable snapshots and inserts them; its factory binds all aggregate lookups and writes to a caller transaction.

## Ownership

- Parent doc: `src/AGENTS.md`.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- No SQLx, provider DTO, or adapter import.

## Verification

- `cargo check -p fxrate-service`
- `cargo test -p fxrate-service --all-features`
