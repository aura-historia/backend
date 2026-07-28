# DOX

## Purpose

- Own `shop-service` crate.
- Own canonical Shop use-case contracts, handlers, and outbound ports for migration.

## Core Design

- Depends on `shop-core` and shared `common` app contracts.
- Root modules: `ports`, `use_case_bundle`, `use_cases`.
- Write handlers use `common::transaction::UnitOfWork` and transaction-scoped repository factories.
- Ports are public because adapter crates implement them.
- No SQLx, DynamoDB, OpenSearch, transport, or legacy `shop` dependency.

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

## Verification

- `cargo check -p shop-service`
- `cargo test -p shop-service --all-features`

## Child DOX Index

- None.
