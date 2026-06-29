# DOX

## Purpose

- Own `product-pipeline-translate` crate.

## Core Design

- Worker Lambda that translates product content for downstream use.
- Root modules: `service`.
- Main neighbors: `common`, `fxrate`, `product`, `shop`.
- Event/runtime edge crate. Keep init and handler glue here, behavior deeper when reusable.

## Ownership

- This doc rule `src/product-pipeline/src/product-pipeline-translate/**`.
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

- `cargo check -p product-pipeline-translate`
- `cargo test -p product-pipeline-translate --all-features`

## Child DOX Index

- None.
