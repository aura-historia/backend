# DOX

## Purpose

- Own bare-metal async worker runtime, Sequin CDC ingestion, and in-memory worker queues for #1341.

## Core Design

- `main.rs` bootstraps logging, config, health/CDC server, and graceful shutdown.
- `lib.rs` owns runtime config, `/health`, `/ready`, `/cdc/sequin`, server loop, and bounded queue primitives.
- `cdc.rs` normalizes Sequin webhook JSON to domain jobs and fans out after route validation.
- `retry.rs` owns in-process retry, idempotency memory, and in-memory DLQ helpers.
- No worker persistence tables in MVP. Crash after CDC fan-out may lose queued jobs.

## Ownership

- This doc rule `src/aura-historia-worker/**`.
- Parent doc: `src/AGENTS.md`.

## Local Contracts

- Read repo root, `src/AGENTS.md`, then here before edit.
- Update this doc when env vars, queue behavior, dependencies, or runtime behavior changes.
- Event-flow changes must update `docs/events/flow.md`.

## Work Guidance

- Keep runtime glue thin.
- Queue payloads should be domain types or domain IDs, not Sequin/AWS envelopes.
- Sub-worker implementation must extract typed DTOs/payloads from Postgres/domain rows when behavior needs event/change fields; do not consume raw Sequin JSON outside router.
- Ack Sequin only after all relevant bounded queue enqueues succeed.
- Use domain idempotency keys; Sequin IDs/LSNs are logs only.
- Keep queue abstraction replaceable by SQS/Lambda/ECS later.

## Verification

- `cargo check -p aura-historia-worker`
- `cargo test -p aura-historia-worker --all-features`

## Child DOX Index

- None.
