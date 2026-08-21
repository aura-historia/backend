# DOX

## Purpose

- Own `notification-service` crate.
- Own canonical notification use cases and service-owned ports.

## Core Design

- Depends on `notification-core` plus pure `money` and `localization` values; never depend on DynamoDB or runtime adapters.
- Root modules:
  - `use_cases/commands` — write use-case handlers/contracts.
  - `use_cases/queries` — read use-case handlers/contracts and dedicated views.
  - `ports` — one file per outbound port; port-local errors/read models live in that port file.
- No compatibility re-export modules.
- No noop adapter in this crate.
- Repository writes return persisted notification state; handlers must not read after write for responses. Conditional notification writers return explicit inserted or already-exists outcomes and a dedicated write error; a deduplicated create result never contains a fabricated notification.
- Keep runtime and HTTP glue outside.

## Ownership

- This doc rule `src/notification-service/**`.
- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo check -p notification-service`
- `cargo test -p notification-service --all-features`

## Child DOX Index

- None.
