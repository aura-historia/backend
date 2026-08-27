# DOX

## Purpose

- Own `notification-service` crate.
- Own canonical notification use cases and service-owned ports.

## Core Design

- Depends on `notification-core` plus pure `money` and `localization` values; never depend on runtime adapters.
- Root modules:
  - `use_cases/commands` — creation and owner-scoped notification mutation handlers/contracts.
  - `use_cases/queries` — list handler/contracts and dedicated views.
  - `ports` — creator, list reader, seen writer, deleter, delivery repository, and channel sender capabilities.
- Notification creation accepts producer-selected external-delivery requests, then the service coordinator plans channel/target intents and persists them atomically for newly inserted notifications only. IDs are generated at the application boundary and outcomes stay input-aligned. The initial planner emits EMAIL/PRIMARY only; producers never select a channel or target. Delivery claims load the persisted channel/target source by delivery ID; `NotificationDeliveryDispatcher` sends through one registered channel sender, captures one completion value, then retries only the matching terminal lease finalization with the original lease token, completion timestamp, and provider receipt/error code. Each channel sender resolves its own target outside generic service code. Duplicate channel registrations and unregistered channels are errors. Seen and delete mutations derive owner only from `OperationContext`; single missing or cross-owner rows return `NotFound`.
- `presentation::NotificationPresentationPreferences` carries language and current `show_unassessed_or_sensitive_content`. The list read model carries canonical `NotificationKind` beside localized content; localization does not own or derive the kind. Watchlist price changes preserve immutable source-currency values, and availability changes preserve optional old/new values; REST and email consume them without FX conversion or changing notification snapshots.
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
