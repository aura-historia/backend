# DOX

## Purpose

- Own `shop-api` crate.

## Core Design

- Shop API Lambda for read, write, and search flows.
- Root modules: `get`, `get_partner_shops`, `patch`, `post`, `search`.
- Main neighbors: `cognito`, `common`, `shop`, `user`.
- HTTP edge crate only. Keep transport here, real rule in domain crates.

## Ownership

- This doc rule `src/shop-api/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, route/event shape, env vars, or child index change.
- If endpoint, auth, payload, or error behavior change, update `docs/swagger.yaml` and `docs/CHANGELOG.md`.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Handler thin: parse, auth, call service, map response.
- No deep business rule in route file.

## Verification

- `cargo check -p shop-api`
- `cargo test -p shop-api --all-features`

## Child DOX Index

- None.
