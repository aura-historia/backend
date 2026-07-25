# DOX

## Purpose

- Own `shop` crate.

## Core Design

- Shop domain, repositories, search, and geocoding services.
- Root modules: `core`, `data`, `dynamodb`, `opensearch`, `service`.

- Main neighbors: `common`, `geo`.
- Library crate. Keep domain, persistence, and service seams explicit.

## Ownership

- This doc rule `src/shop/**`.
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

- `cargo check -p shop`
- `cargo test -p shop --all-features`

## Child DOX Index

- None.
