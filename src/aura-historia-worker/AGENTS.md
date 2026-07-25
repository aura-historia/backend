# DOX

## Purpose

- Own bare-metal async worker runtime skeleton for #1341.

## Core Design

- `main.rs` bootstraps logging, config, health server, and graceful shutdown.
- `lib.rs` owns runtime config, health/readiness endpoints, server loop, and bounded in-memory queue primitives.
- Worker consumes future Sequin CDC, routes domain changes to in-memory queues, then sub-workers handle retries/DLQ in process.
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
- Keep queue abstraction replaceable by SQS/Lambda/ECS later.

## Verification

- `cargo check -p aura-historia-worker`
- `cargo test -p aura-historia-worker --all-features`

## Child DOX Index

- None.
