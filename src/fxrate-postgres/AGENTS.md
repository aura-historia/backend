# DOX

## Purpose

- Own `fxrate-postgres` SQLx FX snapshot repository.

## Core Design

- PostgreSQL is authoritative for immutable snapshots and quotes. Canonical capture serializes inserts and rejects retroactive or tied capture times except duplicate source IDs.
- Rows and SQL stay private. Insert and all quote rows use one caller transaction.
- Repository maps checked persisted rows into core snapshots; its factory binds all aggregate work to caller-owned short or write transactions. No pooled latest-snapshot reader exists.

## Ownership

- Parent doc: `src/AGENTS.md`.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- No product ownership here.

## Verification

- `cargo check -p fxrate-postgres`
- `cargo test -p fxrate-postgres --all-features`
