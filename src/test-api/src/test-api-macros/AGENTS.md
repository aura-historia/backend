# DOX

## Purpose

- Own `test-api-macros` crate.

## Core Design

- Macros for test-api helpers and fixtures.
- `#[aura_integration_test]` is primary integration-test macro; `#[aura_integration_test]` remains legacy alias.
- Macro helper crate. Keep expansion surface tiny and obvious.

## Ownership

- This doc rule `src/test-api/src/test-api-macros/**`.
- Parent doc: `src/test-api/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/test-api/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, route/event shape, env vars, or child index change.
- Macro surface be small and stable. Breaking helper syntax is contract change.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Keep macro magic low. Error output should help fast.

## Verification

- `cargo check -p test-api-macros`
- `cargo test -p test-api-macros --all-features`

## Child DOX Index

- None.
