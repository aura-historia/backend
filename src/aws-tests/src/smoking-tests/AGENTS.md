# DOX

## Purpose

- Own `smoking-tests` crate.

## Core Design

- Smoke tests for deployed or provisioned environments.
- Child crates: `smoking-tests-macros`.
- Main neighbors: `aws-tests-common`, `smoking-tests-macros`.
- Test crate. Favor stable helpers and black-box assertions.

## Ownership

- This doc rule `src/aws-tests/src/smoking-tests/**`.
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

- `cargo check -p smoking-tests`
- `cargo test -p smoking-tests --all-features`

## Child DOX Index

- `src/aws-tests/src/smoking-tests/src/smoking-tests-macros/AGENTS.md` — `smoking-tests-macros` crate.
