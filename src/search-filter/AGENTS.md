# DOX

## Purpose

- Own `search-filter` crate.

## Core Design

- Saved search filter domain, repositories, and match logic.
- Root modules: `core`, `data`, `dynamodb`, `opensearch`, `service`.

- Main neighbors: `common`, `embedding`, `geo`, `product`, `shop`, `user`.
- Library crate. Keep domain, persistence, and service seams explicit.
- A search-filter is a saved search to alert user on new/updated desired products

## Ownership

- This doc rule `src/search-filter/**`.
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

- `cargo check -p search-filter`
- `cargo test -p search-filter --all-features`

## Child DOX Index

- None.
