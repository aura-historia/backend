# DOX

## Purpose

- Own `notification` crate.

## Core Design

- Notification domain, templates, storage, and delivery orchestration.
- Root modules: `core`, `data`, `dynamodb`, `service`.
- Main neighbors: `common`, `product`, `search-filter`, `user`.
- Library crate. Keep domain, persistence, and service seams explicit.
- Every internal notification may be sent externally (e.g. email)

## Ownership

- This doc rule `src/notification/**`.
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

- `cargo check -p notification`
- `cargo test -p notification --all-features`

## Child DOX Index

- None.
