# DOX

## Purpose

- Own `oauth` crate.

## Core Design

- OAuth domain, storage, and service logic.
- Root modules: `core`, `data`, `dynamodb`, `service`.
- Main neighbors: `common`, `user`.
- Library crate. Keep domain, persistence, and service seams explicit.

## Ownership

- This doc rule `src/oauth/**`.
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

- `cargo check -p oauth`
- `cargo test -p oauth --all-features`

## Child DOX Index

- None.
