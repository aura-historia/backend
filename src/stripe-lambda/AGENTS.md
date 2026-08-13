# DOX

## Purpose

- Own `stripe-lambda` crate.

## Core Design

- Event worker that maps Stripe subscription events into canonical User service commands.
- Main neighbors: `common`, `user-service`, `user-postgres`.
- Lambda connects to Postgres directly; it does not use legacy User/DynamoDB services.
- Event/runtime edge crate. Keep init and handler glue here, behavior deeper when reusable.

## Ownership

- This doc rule `src/stripe-lambda/**`.
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

- `cargo check -p stripe-lambda`
- `cargo test -p stripe-lambda --all-features`

## Child DOX Index

- None.
