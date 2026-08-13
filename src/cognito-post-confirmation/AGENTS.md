# DOX

## Purpose

- Own `cognito-post-confirmation` crate.

## Core Design

- Cognito trigger that finishes user setup after signup.
- Main neighbors: `common`, `user-service`, `user-postgres`.
- Event/runtime edge crate. Map Cognito `sub` and `email` into `CreateUserUseCase` under `Principal::System`; Postgres is canonical user truth.
- Cognito may redeliver. Same subject/email must be idempotent; mismatched replay and unresolved service failures stay retry-visible.

## Ownership

- This doc rule `src/cognito-post-confirmation/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, route/event shape, env vars, or child index change.
- If trigger, retry, env var, queue/topic, or side effect change, update `infra/` and test wiring too.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Bootstrap thin. Push reusable work into service or domain crate.
- Be clear about event source, idempotency, and side effects.

## Verification

- `cargo check -p cognito-post-confirmation`
- `cargo test -p cognito-post-confirmation --all-features`

## Child DOX Index

- None.
