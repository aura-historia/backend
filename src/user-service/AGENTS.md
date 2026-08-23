# DOX

## Purpose

- Own `user-service` crate.
- Own canonical User use-case contracts, handlers, and outbound ports.

## Core Design

- Depends on `user-core` identifiers and values, pure `money`/`localization` values, and shared `application` contracts.
- Root modules: `ports`, `use_case_bundle`, `use_cases`.
- `use_cases::authorization` owns shared service-layer admin actor policy helpers.
- Admin actor checks use transaction-scoped `UserAdminReader::find_admin_actor`, not controller checks.
- Own-user reads and admin-user reads are separate use cases: `GetOwnUserUseCase` and `AdminGetUserUseCase`.
- Operational handlers use `application::transaction::UnitOfWork` and transaction-scoped repository/reader factories.
- User read/search/update/delete use cases authorize self where allowed, service/system, or admin actor in service layer.
- Repository writes return persisted user state; handlers must not read after write for responses.
- Ports are public because adapter crates implement them.
- `UserTierEntitlements` locks one authoritative user row and reconciles tier-restricted search filters and watchlist entries inside the caller transaction; it avoids a User-service dependency on either resource service.
- `AuthenticateAccessTokenUseCase` only validates token existence/expiry and returns token scopes; protected use cases enforce credential capability via `OperationContext`.
- Port errors carry boxed sources for adapter/read-model failures; do not swallow underlying causes.
- No SQLx, DynamoDB, OpenSearch, or transport dependency.

## Ownership

- This doc rule `src/user-service/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, dependency edge, or use-case boundary changes.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Keep orchestration here. Keep rules in `user-core`.
- Keep adapters outside.
- Keep unit tests inside the use-case file that owns the handler. No shared test-support module.

## Verification

- `cargo check -p user-service`
- `cargo test -p user-service --all-features`

## Child DOX Index

- None.
