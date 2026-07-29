# DOX

## Purpose

- Own `user-service` crate.
- Own canonical User use-case contracts and outbound ports for migration.

## Core Design

- Depends on `user-core` and shared `common` app contracts.
- Root modules: `ports`, `use_case_bundle`, `use_cases`.
- Operational handlers use `common::transaction::UnitOfWork` and transaction-scoped repository/reader factories when added.
- Ports are public because adapter crates implement them.
- Port errors carry boxed sources for adapter/read-model failures; do not swallow underlying causes.
- No SQLx, DynamoDB, OpenSearch, transport, or legacy `user` dependency.

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

## Verification

- `cargo check -p user-service`
- `cargo test -p user-service --all-features`

## Child DOX Index

- None.
