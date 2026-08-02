# DOX

## Purpose

- Own canonical OAuth use cases and outbound ports.

## Core Design

- One OAuth use-case module per command/query.
- `ports/` has one module per aggregate/read capability: clients, authorization codes, third-party exchange codes, and access-token gateway.
- `access_token_gateway.rs` adapts the User access-token store for OAuth token issuance, lookup, and revocation.
- No DynamoDB, HTTP, Lambda, or storage records.

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
