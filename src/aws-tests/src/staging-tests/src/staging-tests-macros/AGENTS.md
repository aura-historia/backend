# DOX

## Purpose

- Own `staging-tests-macros` crate.

## Core Design

- Test macros for staging suites.
- Macro helper crate. Keep expansion surface tiny and obvious.

## Ownership

- This doc rule `src/aws-tests/src/staging-tests/src/staging-tests-macros/**`.
- Parent doc: `src/aws-tests/src/staging-tests/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/aws-tests/src/staging-tests/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, route/event shape, env vars, or child index change.
- Macro surface be small and stable. Breaking helper syntax is contract change.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Keep macro magic low. Error output should help fast.

## Verification

- `cargo check -p staging-tests-macros`
- `cargo test -p staging-tests-macros --all-features`

## Child DOX Index

- None.
