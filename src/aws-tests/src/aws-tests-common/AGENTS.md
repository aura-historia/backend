# DOX

## Purpose

- Own `aws-tests-common` crate.

## Core Design

- Shared AWS and LocalStack test helpers.
- Test crate. Favor stable helpers and black-box assertions.

## Ownership

- This doc rule `src/aws-tests/src/aws-tests-common/**`.
- Parent doc: `src/aws-tests/AGENTS.md`.
- No child doc below.

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

- `cargo check -p aws-tests-common`
- `cargo test -p aws-tests-common --all-features`

## Child DOX Index

- None.
