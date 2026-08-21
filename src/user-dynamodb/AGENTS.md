# DOX

## Purpose

- Own `user-dynamodb` crate.
- Implement canonical User DynamoDB adapters.

## Core Design

- Depends on `user-service`, `user-core`, `credential-core`, `application`, and AWS SDK.
- Implements `user-service` ports.
- Owns access-token DynamoDB record shape, update expressions, and mapping, including canonical scope records.
- Storage records stay inside adapter boundary and never escape service ports.

## Ownership

- This doc rule `src/user-dynamodb/**`.
- Parent doc: `src/AGENTS.md`.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- Update this file when adapter purpose, storage shape, dependency edge, or test flow changes.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Map AWS/storage errors to `user-service` port errors.
- Treat corrupt persisted records as `InvalidPersistedState`; do not skip silently.

## Verification

- `cargo check -p user-dynamodb`
- `cargo test -p user-dynamodb --all-features`

## Child DOX Index

- None.
