# DOX

## Purpose

- Own `shop-service` crate.
- Own canonical Shop use-case contracts, handlers, and outbound ports.

## Core Design

- Depends on `shop-core`, shared `application` contracts, pure `money`/`localization` values, and shared `geo::{Geocoder, GeocodingError}`.
- Root modules: `ports`, `shop_search`, `use_case_bundle`, `use_cases`. `shop_search::ShopSearch` is the Shop search use-case input.
- Operational handlers use `application::transaction::UnitOfWork` and transaction-scoped repository/reader factories.
- Ports are public because adapter/runtime crates implement them.
- Shop write use cases own admin/partner authorization checks inline; controllers must not enforce those rules. WooCommerce ports expose transaction-scoped partner membership, safe integration config, and HMAC verification for the Product intake use case. The verifier returns semantic outcomes; raw secrets never appear in Shop service application return types.
- Shop admin checks call `CheckUserAdminUseCase` with actor from `OperationContext`, not target user id.
- Repository writes return persisted storage-neutral state; write use cases must not read after write to build responses.
- Query use cases return read-optimized payloads for their API use case and avoid controller-side N+1 hydration.
- Port errors carry boxed sources for adapter/read-model failures; do not swallow underlying causes.
- No SQLx, OpenSearch, or transport dependency.

## Ownership

- This doc rule `src/shop-service/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, dependency edge, or use-case boundary changes.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Keep orchestration here. Keep rules in `shop-core`.
- Keep adapters outside.
- Keep unit tests inside the use-case file that owns the handler. No shared test-support module. Do not put plain Tokio unit tests in `tests/`.

## Verification

- `cargo check -p shop-service`
- `cargo test -p shop-service --all-features`

## Child DOX Index

- None.
