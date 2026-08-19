# DOX

## Purpose

- Own `notification-service` crate.
- Own canonical notification use cases and service-owned ports.

## Core Design

- Depends on `notification-core`; never depend on DynamoDB or runtime adapters.
- Root modules:
  - `use_cases/commands` — creation and owner-scoped notification mutation handlers/contracts.
  - `use_cases/queries` — list handler/contracts and dedicated views.
  - `ports` — creator, list reader, seen writer, deleter, delivery repository, and delivery sender capabilities.
- Notification creation accepts producer-selected external-delivery requests, then the service coordinator plans channel/target intents and persists them atomically for newly inserted notifications only. IDs are generated at the application boundary and outcomes stay input-aligned. The initial planner emits EMAIL/PRIMARY only; producers never select a channel or target. Delivery claims, sending, and lease finalization belong to the focused delivery use case. Seen and delete mutations derive owner only from `OperationContext`; single missing or cross-owner rows return `NotFound`.
- No compatibility re-export modules or noop adapters.
- Keep runtime and HTTP glue outside.

## Ownership

- This doc rule `src/notification-service/**`.
- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo check -p notification-service`
- `cargo test -p notification-service --all-features`

## Child DOX Index

- None.
