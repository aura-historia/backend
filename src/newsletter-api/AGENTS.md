# DOX

## Purpose

- Own `newsletter-api` crate.

## Core Design

- API Lambda for newsletter subscription flow through Zoho and user data.
- Root modules: `data`, `domain`, `put`, `service`.
- Main neighbors: `cognito`, `common`, `user`.
- HTTP edge crate only. Keep transport here, real rule in domain crates.

## Ownership

- This doc rule `src/newsletter-api/**`.
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

- `cargo check -p newsletter-api`
- `cargo test -p newsletter-api --all-features`

## Child DOX Index

- None.
