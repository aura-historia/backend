---
name: aura-rust-test
description: Use when adding or changing Aura Historia Rust tests, choosing test placement, validating architecture changes, testing adapters/controllers/use cases, or deciding cargo/npm verification commands.
---

# Aura Rust Test

Use for test placement and validation.

## Must read

- `backend/AGENTS.md` and path `AGENTS.md` files.
- `docs/arch.md` §5.5, §20, §23-24.

## Placement rules

- Tests that need private or `pub(crate)` details live beside implementation under `#[cfg(test)] mod tests`.
- Crate-level `/tests` is for black-box tests of deliberate public API only.
- Do not make implementation details public just for `/tests`.
- Real-infrastructure adapter tests may live beside implementation.

## What to test

- Core: valid transitions, rejected transitions, invariants, event emission when used, idempotent no-op behavior.
- Service: orchestration, begin/commit, same transaction across factories, batching, fallback, optional enrichment, search order, error translation, authorization, no-op persistence skip.
- PostgreSQL adapter: `FromRow`, aggregate rehydration, insert/update semantics, optimistic concurrency, rollback, cross-entity transactions, joined readers, migration compatibility.
- Other adapters: request serialization, response deserialization, application mapping, timeout/error mapping, stale-version handling.
- Controllers: DTO deserialization, request/use-case mapping, status mapping, response serialization, auth/context mapping, missing-token behavior on public routes, invalid-token rejection, protected-route auth.
- Acceptance: externally visible behavior through the public runtime boundary. REST flows use the public HTTP API; worker flows use real source commit → Sequin webhook → running worker HTTP server → real target store. One file per endpoint or worker route group.
- Every production worker needs complete real-infrastructure acceptance coverage in its runtime crate `tests/`: committed happy paths, source rollback, ignored input, duplicate/redelivery idempotency, filtering, and target payload/result shape. Use real Postgres, Sequin, and each written target store; mocks do not satisfy this rule.

## API test rules

- Controller tests should exercise axum `Router` with fake inbound use-case traits and fake authenticators.
- Mock inbound use-case traits, not repositories.
- For `aura-historia-api` black-box tests, use `test-api::AuraHistoriaApi` as a process-lived test service.
- Acceptance tests for authenticated routes should use Aura Historia access tokens when public contract supports them.

## Validation commands

Start targeted, then broaden when useful:

```sh
cargo check --workspace
cargo depgraph-check check
cargo test --workspace --lib --all-features
npm --prefix infra test
npm --prefix infra run synth:all
```

Run infra commands only when infra changed or user asks.

## Before completion

- Check no visibility widened solely for tests.
- Check no N+1 access pattern.
- Check Cargo dependencies preserve core <- service <- adapters <- runtime/transport.
- Check no service/core imports adapter.
- Check no controller accesses repository or database client.
- Check logs contain no secrets or sensitive payloads.
- State exactly which validation ran and result.
