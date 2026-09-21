# DOX

## Purpose

- Own `fxrate-lambda` crate.

## Core Design

- Scheduled and deployment-bootstrap Lambda that captures immutable canonical FX snapshots in Postgres.
- Main neighbors: `application`, `platform-observability`, `platform-postgres`, `fxrate-service`, `fxrate-postgres`, `fxrate-fxratesapi`.
- Event/runtime edge crate. It maps EventBridge IDs to idempotency keys and wires canonical FX service/Postgres/provider adapters; behavior stays deeper.
- PostgreSQL needs `POSTGRES_TLS_ROOT_CERT`; real stages refresh `POSTGRES_SECRET_ARN` `AWSCURRENT` before each trigger and cache a full versioned handler lease. Ephemeral keeps fixture username/password. Bootstrap uses the strict min-zero/default-max-one verified-TLS pool profile.

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
