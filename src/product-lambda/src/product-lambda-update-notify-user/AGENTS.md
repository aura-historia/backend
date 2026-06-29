# DOX

## Purpose

- Own `product-lambda-update-notify-user` crate.

## Core Design

- Worker Lambda that emits user notification work from product changes.
- Root modules: `service`.
- Main neighbors: `common`, `notification`, `product`, `product-watchlist`, `user`.
- Event/runtime edge crate. Keep init and handler glue here, behavior deeper when reusable.

## Ownership

- This doc rule `src/product-lambda/src/product-lambda-update-notify-user/**`.
- Parent doc: `src/product-lambda/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/product-lambda/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, route/event shape, env vars, or child index change.
- If trigger, retry, env var, queue/topic, or side effect change, update `infra/` and test wiring too.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Bootstrap thin. Push reusable work into service or domain crate.
- Be clear about event source, idempotency, and side effects.

## Verification

- `cargo check -p product-lambda-update-notify-user`
- `cargo test -p product-lambda-update-notify-user --all-features`

## Child DOX Index

- None.
