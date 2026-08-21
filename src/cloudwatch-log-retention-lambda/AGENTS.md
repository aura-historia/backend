# DOX

## Purpose

- Own `cloudwatch-log-retention-lambda` crate.

## Core Design

- Lambda that applies log retention policy to CloudWatch groups.
- Main neighbor: `platform-observability`.
- Event/runtime edge crate. Keep init and handler glue here, behavior deeper when reusable.

## Ownership

- This doc rule `src/cloudwatch-log-retention-lambda/**`.
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

- `cargo check -p cloudwatch-log-retention-lambda`
- `cargo test -p cloudwatch-log-retention-lambda --all-features`

## Child DOX Index

- None.
