# DOX

## Purpose

- Own `shop-lambda-opensearch-index` crate.

## Core Design

- Worker Lambda that projects shops into OpenSearch.
- Main neighbors: `common`, `shop`.
- Event/runtime edge crate. Keep init and handler glue here, behavior deeper when reusable.

## Ownership

- This doc rule `src/shop-lambda/src/shop-lambda-opensearch-index/**`.
- Parent doc: `src/shop-lambda/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/shop-lambda/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, route/event shape, env vars, or child index change.
- If trigger, retry, env var, queue/topic, or side effect change, update `infra/` and test wiring too.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Bootstrap thin. Push reusable work into service or domain crate.
- Be clear about event source, idempotency, and side effects.

## Verification

- `cargo check -p shop-lambda-opensearch-index`
- `cargo test -p shop-lambda-opensearch-index --all-features`

## Child DOX Index

- None.
