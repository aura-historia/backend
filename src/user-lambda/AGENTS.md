# DOX

## Purpose

- Own `user-lambda` crate.

## Core Design

- Parent crate for async user workers.
- Child crates: `user-lambda-index-opensearch`, `user-lambda-tier-update`.
- Main neighbors: `user-lambda-index-opensearch`, `user-lambda-tier-update`.
- Parent crate exists to group child executables or suites and keep their map discoverable.

## Ownership

- This doc rule `src/user-lambda/**`.
- Parent doc: `src/AGENTS.md`.
- Child docs below rule deeper child crates.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, route/event shape, env vars, or child index change.
- Keep child crate list honest. Shared parent glue stay tiny.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Parent crate own map and shared glue. Real work live in child crates.

## Verification

- `cargo check -p user-lambda`

## Child DOX Index

- `src/user-lambda/src/user-lambda-index-opensearch/AGENTS.md` — `user-lambda-index-opensearch` crate.
- `src/user-lambda/src/user-lambda-tier-update/AGENTS.md` — `user-lambda-tier-update` crate.
