# DOX

## Purpose

- Own `staging-tests` crate.

## Core Design

- Staging environment behavior tests.
- Child crates: `staging-tests-macros`.
- Test crate. Favor stable helpers and black-box assertions.

## Ownership

- This doc rule `src/aws-tests/src/staging-tests/**`.
- Parent doc: `src/aws-tests/AGENTS.md`.
- Child docs below rule deeper child crates.

## Local Contracts

- Read `AGENTS.md`, `src/aws-tests/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, route/event shape, env vars, or child index change.
- Keep fixtures deterministic. Add or move suite paths in `src/ci-determinator` when CI scope change.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Tests prove behavior, not implementation trivia.
- Share helpers before copy-paste fixtures.

## Verification

- `cargo check -p staging-tests`
- `cargo test -p staging-tests --all-features`

## Child DOX Index

- `src/aws-tests/src/staging-tests/src/staging-tests-macros/AGENTS.md` — `staging-tests-macros` crate.
