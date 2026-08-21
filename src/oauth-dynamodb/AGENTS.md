# DOX

## Purpose

- Own canonical OAuth DynamoDB adapter.

## Core Design

- Map OAuth domain types to legacy DynamoDB table records.
- Implement OAuth service repository and reader ports.
- Keep AWS SDK, record shapes, update expressions, and serialization helpers here; these storage mechanics do not escape the adapter.

## Ownership

- This doc rule `src/oauth-dynamodb/**`.
- Parent doc: `src/AGENTS.md`.

## Local Contracts

- Read repo root, `src/AGENTS.md`, then here before edit.
- Update this doc and `docs/dynamodb/table_1.md` when record shape changes.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- No service rule here. No storage record escape.

## Verification

- `cargo check -p oauth-dynamodb`
- `cargo test -p oauth-dynamodb --all-features`

## Child DOX Index

- None.
