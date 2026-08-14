# DOX

## Purpose

- Own `fxrate-lambda` crate.

## Core Design

- Scheduled Lambda that captures immutable Product FX snapshots in Postgres.
- Main neighbors: `common`, `product-service`, `product-postgres`, `product-fxratesapi`.
- Event/runtime edge crate. It maps EventBridge IDs to idempotency keys and wires Product service/Postgres/provider adapters; behavior stays deeper.

## Ownership

- This doc rule `src/fxrate-lambda/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, route/event shape, env vars, or child index change.
- If trigger, retry, env var, queue/topic, or side effect change, update `infra/` and test wiring too.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Bootstrap thin. Push reusable work into service or domain crate.
- Be clear about event source, idempotency, and side effects.

## Verification

- `cargo check -p fxrate-lambda`
- `cargo test -p fxrate-lambda --all-features`

## Child DOX Index

- None.
