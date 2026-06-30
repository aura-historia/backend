# DOX

## Purpose

- Own `acceptance-tests` crate.

## Core Design

- Full-stack acceptance tests for CDK-synthesized backend behavior.
- Test crate. Favor stable helpers and black-box assertions.
- Painfully slow execution.
- Tests core behavior, usage-paths, integration with other crates

## Ownership

- This doc rule `src/acceptance-tests/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, route/event shape, env vars, or child index change.
- Keep fixtures deterministic. Add or move suite paths in `src/ci-determinator` when CI scope change.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Tests prove behavior, not implementation trivia.
- Share helpers before copy-paste fixtures.

## Verification

- `cargo check -p acceptance-tests`
- `cargo test -p acceptance-tests --all-features`

## Child DOX Index

- None.
