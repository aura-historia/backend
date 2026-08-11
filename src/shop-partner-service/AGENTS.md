# DOX

## Purpose

- Own `shop-partner-service` crate.
- Own partner shop application use cases, ports, admin application flow, and partner shop list query.

## Core Design

- Depends on `shop-partner-core`, `shop-core`, `shop-service`, `user-core`, `user-service`, and `common` app contracts.
- Root modules: `ports`, `use_cases`, `use_case_bundle`.
- Handlers use `UnitOfWork` and transaction-scoped repository/reader factories.
- New shop application creates a draft shop first, then the application row. Approval publishes the linked shop in the same transaction; rejection and withdrawal leave it hidden.
- Own application use cases require owner or service/system context.
- Admin application use cases allow service/system, or persisted admin users checked through `UserAdminReader::find_admin_actor`.
- Repository writes return persisted partner-shop application state.

## Ownership

- This doc rule `src/shop-partner-service/**`.
- Parent doc: `src/AGENTS.md`.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Keep orchestration here. Keep SQL outside.

## Verification

- `cargo check -p shop-partner-service`
- `cargo test -p shop-partner-service --all-features`

## Child DOX Index

- None.
