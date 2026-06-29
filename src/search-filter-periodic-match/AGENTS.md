# DOX

## Purpose

- Own `search-filter-periodic-match` crate.

## Core Design

- Scheduled matcher that re-runs saved filter matching as hybrid-search.
- Main neighbors: `common`, `notification`, `product`, `search-filter`, `user`.
- Event/runtime edge crate. Keep init and handler glue here, behavior deeper when reusable.

## Ownership

- This doc rule `src/search-filter-periodic-match/**`.
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

- `cargo check -p search-filter-periodic-match`
- `cargo test -p search-filter-periodic-match --all-features`

## Child DOX Index

- None.
