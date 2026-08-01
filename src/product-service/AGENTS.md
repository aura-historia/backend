# DOX

## Purpose

- Own `product-service` crate.
- Own canonical Product use-case contracts, handlers, and outbound ports for migration.

## Core Design

- Depends on `product-core` and shared `common` app contracts.
- Root modules: `ports`, `use_case_bundle`, `use_cases`.
- Write handlers use `common::transaction::UnitOfWork` and transaction-scoped repository/event-store factories.
- Repository writes return persisted product state; handlers must not read after write for responses.
- OpenSearch-backed search is an ordinary reader. Do not model it as transactional.
- Ports are public because adapter crates implement them.
- Port errors carry boxed sources for adapter/read-model failures; do not swallow underlying causes.
- No SQLx, DynamoDB, OpenSearch, transport, or legacy `product` dependency.

## Ownership

- This doc rule `src/product-service/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, dependency edge, or use-case boundary changes.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Keep orchestration here. Keep rules in `product-core`.
- Keep adapters outside.
- Keep unit tests inside the use-case file that owns the handler. No shared test-support module.

## Verification

- `cargo check -p product-service`
- `cargo test -p product-service --all-features`

## Child DOX Index

- None.
