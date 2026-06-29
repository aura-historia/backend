# DOX

## Purpose

- Own `partner-shop-application-lambda` crate.

## Core Design

- Async Lambda that turns approved partner applications into shops and notifications.
- Main neighbors: `common`, `geo`, `notification`, `partner-shop-application`, `shop`, `user`.
- Event/runtime edge crate. Keep init and handler glue here, behavior deeper when reusable.

## Ownership

- This doc rule `src/partner-shop-application-lambda/**`.
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

- `cargo check -p partner-shop-application-lambda`
- `cargo test -p partner-shop-application-lambda --all-features`

## Child DOX Index

- None.
