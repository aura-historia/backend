# DOX

## Purpose

- Own legacy `user` crate.

## Core Design

- Legacy User domain, DynamoDB repositories, OpenSearch projection, and services.
- Root modules: `core`, `data`, `dynamodb`, `opensearch`, `service`.
- Canonical migration types now live in `user-core` and `user-service`.
- Old `core::user::User` and `service::{command,user_service}` stay until cutover.

- Main neighbors: `common`, `geo`.
- Library crate. Keep old behavior stable while migration moves canonical code out.

## Ownership

- This doc rule `src/user/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when legacy crate contract, route/event shape, env vars, or child index change.
- Do not add new canonical migration contracts here.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Service and repository split stay clean.
- Keep transport and runtime glue out of domain core.

## Verification

- `cargo check -p user`
- `cargo test -p user --all-features`

## Child DOX Index

- None.
