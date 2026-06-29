# DOX

## Purpose

- Own `product-pipeline-embed-text` crate.

## Core Design

- Worker Lambda that builds text embeddings for search and matching.
- Root modules: `service`.
- Main neighbors: `common`, `fxrate`, `product`, `shop`.
- Event/runtime edge crate. Keep init and handler glue here, behavior deeper when reusable.

## Ownership

- This doc rule `src/product-pipeline/src/product-pipeline-embed-text/**`.
- Parent doc: `src/product-pipeline/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/product-pipeline/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, route/event shape, env vars, or child index change.
- If trigger, retry, env var, queue/topic, or side effect change, update `infra/` and test wiring too.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Bootstrap thin. Push reusable work into service or domain crate.
- Be clear about event source, idempotency, and side effects.

## Verification

- `cargo check -p product-pipeline-embed-text`
- `cargo test -p product-pipeline-embed-text --all-features`

## Child DOX Index

- None.
