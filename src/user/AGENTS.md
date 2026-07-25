# DOX

## Purpose

- Own `user` crate.

## Core Design

- User domain, repositories, search projection, and services.
- Root modules: `core`, `data`, `dynamodb`, `opensearch`, `service`.
- `migrations/` holds target Postgres user schema. Access tokens stay in DynamoDB.
- Main neighbors: `common`, `geo`.
- Library crate. Keep domain, persistence, and service seams explicit.

## Ownership

- This doc rule `src/user/**`.
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

- `cargo check -p user`
- `cargo test -p user --all-features`

## Child DOX Index

- None.
