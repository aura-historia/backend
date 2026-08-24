# DOX

## Purpose

- Own canonical OAuth use cases and outbound ports.

## Core Design

- One OAuth use-case module per command/query.
- `ports/` owns transaction-scoped OAuth aggregate repositories, including one-time code repositories that only insert or atomically consume, plus purpose-specific client details/list readers returning `OAuthClientView` and a narrow client-authentication reader that exposes secret hash material only to OAuth service handlers.
- OAuth token issue/revoke flows compose public User repository contracts in the same PostgreSQL transaction.
- No HTTP, Lambda, or storage records.

## Ownership

- This doc rule `src/oauth-service/**`.
- Parent doc: `src/AGENTS.md`.

## Local Contracts

- Read repo root, `src/AGENTS.md`, then here before edit.
- Update this doc when use case, port, auth behavior, or dependency edge changes.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- No god service. Split use cases.
- No secret payload logging.

## Verification

- `cargo check -p oauth-service`
- `cargo test -p oauth-service --all-features`

## Child DOX Index

- None.
