# DOX

## Purpose

- Own `user-lambda-tier-update` crate.

## Core Design

- Worker Lambda that recalculates user tier state.
- Main neighbors: `common`, `product-watchlist`, `search-filter`, `user`.
- Event/runtime edge crate. Keep init and handler glue here, behavior deeper when reusable.

## Ownership

- This doc rule `src/user-lambda/src/user-lambda-tier-update/**`.
- Parent doc: `src/user-lambda/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/user-lambda/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, route/event shape, env vars, or child index change.
- If trigger, retry, env var, queue/topic, or side effect change, update `infra/` and test wiring too.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Bootstrap thin. Push reusable work into service or domain crate.
- Be clear about event source, idempotency, and side effects.

## Verification

- `cargo check -p user-lambda-tier-update`
- `cargo test -p user-lambda-tier-update --all-features`

## Child DOX Index

- None.
