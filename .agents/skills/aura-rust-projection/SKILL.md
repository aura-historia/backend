---
name: aura-rust-projection
description: Use when adding or changing Aura Historia Rust CDC, Sequin routing, projection jobs, OpenSearch or key-value projections, replay/rebuild paths, projection mappings, or CDC tests.
---

# Aura Rust Projection

Use for CDC and rebuildable read projections.

## Must read

- `backend/AGENTS.md` and path `AGENTS.md` files.
- `docs/arch.md` §10.4, §12, §14, §17, §20.6, §21-23.

## Before coding

- Identify operational owner of every dataset.
- Classify each store: authoritative storage, rebuildable projection, external source, or cache.
- Identify source version/idempotency strategy.
- Identify replay/rebuild path and verification.
- Update projection docs when behavior, ownership, rebuild, or delivery guarantee changes.

## Hard rules

- Every dataset has one documented operational owner.
- PostgreSQL owns business truth unless a bounded context documents another owner.
- OpenSearch contains rebuildable search projections only.
- Projection stores are never part of PostgreSQL transactions.
- Only committed PostgreSQL changes are propagated.
- Domain invariants must not depend on projections being current.
- CDC router validates change shape, derives stable job IDs, enqueues all required jobs, applies bounded backpressure, then acknowledges or rejects Sequin delivery.
- Acknowledge Sequin only after all required jobs are added to bounded in-memory queues.
- If any enqueue fails, do not acknowledge.
- Redelivery may create duplicate jobs; all handlers and projection writes must be idempotent.
- Current MVP post-ack jobs are in memory and may be lost on worker death. Do not claim exactly-once or durable at-least-once.
- Projection records should store latest applied source version.
- Older or equal source version must not overwrite newer projection state.
- Prefer target-side conditional updates, unique constraints, or version checks over in-memory duplicate checks.
- Treat incomplete CDC payloads as invalidation signals: read current committed authoritative state, build full projection, conditionally update target.
- Joined/hydrated projections should reread authoritative state instead of incrementally merging unrelated partial changes.
- Projection mapping belongs to target adapter.
- Search documents/items must not escape their adapter.
- Existing projections are not recovery source for authoritative data.
- Poison changes are never silently discarded.

## Observability

- Monitor replication lag, WAL growth, Sequin lag/retries, unacknowledged age, router failures, queue depth, handler failures/latency, duplicate/stale rejections, projection freshness, and rebuild status.
- Logs include safe identifiers, source/table/op, version, job type, idempotency key, attempt, outcome, and correlation id.
- Never log complete source rows, credentials, tokens, or sensitive payloads.

## Tests

- Cover insert/update/delete mapping, duplicate delivery, concurrent delivery, stale changes, partial enqueue then redelivery, queue saturation, projection version checks, replay, and full rebuild.
- Every production worker route MUST have a black-box acceptance suite in its runtime crate `tests/`. Use real Postgres, real Sequin webhook delivery, the running worker HTTP server, and every real target store it writes (for example OpenSearch). Do not replace this flow with mocked ports.
- Worker acceptance cases MUST cover committed happy paths, source rollback, ignored/unrouted changes, redelivery/target idempotency, recipient or projection filtering, and persisted target payload shape. Test retryable queue/backpressure and malformed CDC behavior at the narrowest suitable layer when a real Sequin setup cannot deterministically induce them.
- Keep MVP crash-after-ack loss window documented or covered by operational test.
