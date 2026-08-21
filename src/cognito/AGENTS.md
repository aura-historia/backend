# DOX

## Purpose

- Own `cognito` crate.

## Core Design

- Cognito token verification and auth helpers.
- Root modules: `access_token_verifier_service`, `localstack_access_token_verifier_service`.
- Main neighbor: `user-core`.
- Library crate. Keep domain, persistence, and service seams explicit.
- Cognito only used as identidy provider. No business logic here. No user-attributes besides. They live in DynamoDB.

## Ownership

- This doc rule `src/cognito/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, route/event shape, env vars, or child index change.
- Keep business rules here, not leaked into callers.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Service and repository split stay clean.
- Keep transport and runtime glue out of domain core.

## Verification

- `cargo check -p cognito`
- `cargo test -p cognito --all-features`

## Child DOX Index

- None.
