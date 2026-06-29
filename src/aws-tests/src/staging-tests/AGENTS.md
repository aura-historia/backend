## Purpose

- Own `staging-tests` crate and child crate map.

## Ownership

- This doc rule `src/aws-tests/src/staging-tests/**`.
- Parent doc: `src/aws-tests/AGENTS.md`.
- Child docs below rule deeper child crates.

## Local Contracts

- Read `AGENTS.md`, `src/aws-tests/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract or child index change.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Match crate pattern. Keep cross-crate bleed low.

## Verification

- `cargo check -p staging-tests`

## Child DOX Index

- `src/aws-tests/src/staging-tests/src/staging-tests-macros/AGENTS.md` — `staging-tests-macros` crate.
