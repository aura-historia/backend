# DOX

## Purpose

- Own `notification-api` crate.

## Core Design

- API Lambda for reading and mutating user notifications.
- Root modules: `notification_delete_all`, `notification_delete_one`, `notification_get`, `notification_patch_all`, `notification_patch_one`.
- Main neighbors: `common`, `notification`, `user`.
- HTTP edge crate only. Keep transport here, real rule in domain crates.

## Ownership

- This doc rule `src/notification-api/**`.
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

- `cargo check -p notification-api`
- `cargo test -p notification-api --all-features`

## Child DOX Index

- None.
