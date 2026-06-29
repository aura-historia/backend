## Purpose

- Own `aws-tests` crate and child crate map.

## Ownership

- This doc rule `src/aws-tests/**`.
- Parent doc: `src/AGENTS.md`.
- Child docs below rule deeper child crates.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract or child index change.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Match crate pattern. Keep cross-crate bleed low.

## Verification

- `cargo check -p aws-tests`

## Child DOX Index

- `src/aws-tests/src/aws-tests-common/AGENTS.md` — `aws-tests-common` crate.
- `src/aws-tests/src/smoking-tests/AGENTS.md` — `smoking-tests` crate.
- `src/aws-tests/src/staging-tests/AGENTS.md` — `staging-tests` crate.
