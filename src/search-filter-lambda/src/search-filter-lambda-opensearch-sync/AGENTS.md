# DOX

## Purpose

- Own `search-filter-lambda-opensearch-sync` crate.

## Core Design

- Worker Lambda that syncs search filter projection into OpenSearch.
- Main neighbors: `common`, `search-filter`.
- Event/runtime edge crate. Keep init and handler glue here, behavior deeper when reusable.

## Ownership

- This doc rule `src/search-filter-lambda/src/search-filter-lambda-opensearch-sync/**`.
- Parent doc: `src/search-filter-lambda/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/search-filter-lambda/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, route/event shape, env vars, or child index change.
- If trigger, retry, env var, queue/topic, or side effect change, update `infra/` and test wiring too.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Bootstrap thin. Push reusable work into service or domain crate.
- Be clear about event source, idempotency, and side effects.

## Verification

- `cargo check -p search-filter-lambda-opensearch-sync`
- `cargo test -p search-filter-lambda-opensearch-sync --all-features`

## Child DOX Index

- None.
