# DOX

## Purpose

- Own `partner-shop-application-api` crate.

## Core Design

- API Lambda for public and admin partner shop application routes.
- Root modules: `admin_decision`, `admin_get_all`, `admin_get_one`, `admin_patch`, `delete`, `get_all`, `get_one`, `patch`, `post`, `path`.
- Main neighbors: `common`, `partner-shop-application`, `shop`, `user`.
- HTTP edge crate only. Keep transport here, real rule in domain crates.

## Ownership

- This doc rule `src/partner-shop-application-api/**`.
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

- `cargo check -p partner-shop-application-api`
- `cargo test -p partner-shop-application-api --all-features`

## Child DOX Index

- None.
