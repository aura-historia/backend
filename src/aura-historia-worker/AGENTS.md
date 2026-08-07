# DOX

## Purpose

- Own bare-metal async worker runtime, Sequin CDC ingestion, and in-memory worker queues for #1341.

## Core Design

- `main.rs` bootstraps logging, config, health/CDC server, and graceful shutdown.
- `lib.rs` owns runtime config, `/health`, `/ready`, `/cdc/sequin`, server loop, default all-queue runtime, and bounded queue primitives.
- `cdc.rs` normalizes Sequin webhook JSON to domain jobs and fans out after route validation.
- `search_filter_projection.rs` consumes `SearchFilterOpenSearch` jobs, rereads committed Postgres state, and writes the canonical OpenSearch projection with target-side version protection.
- `retry.rs` owns in-process retry, idempotency memory, and in-memory DLQ helpers.
- No worker persistence tables in MVP. Crash after CDC fan-out may lose queued jobs.

## Ownership

- This doc rule `src/aura-historia-worker/**`.
- Parent doc: `src/AGENTS.md`.

## Local Contracts

- Read repo root, `src/AGENTS.md`, then here before edit.
- Update this doc when env vars, queue behavior, dependencies, or runtime behavior changes.
- Runtime accepts only `search_filters` CDC; other tables fail delivery rather than filling unconsumed queues. Configure Sequin accordingly.
- Runtime requires `POSTGRES_*`, `OPENSEARCH_ENDPOINT_URL`, and outside `STAGE=ephemeral`, OpenSearch credentials.
- Event-flow changes must update `docs/events/flow.md`.

## Work Guidance

- Keep runtime glue thin.
- Register all known worker queues by default when every route has a consumer. A dedicated runtime may register only its explicitly scoped CDC route; it must reject other tables before acknowledgment.
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
