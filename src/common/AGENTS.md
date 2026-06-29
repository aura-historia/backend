## Purpose

- Own `common` crate.

## Ownership

- This doc rule `src/common/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract or child index change.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Match crate pattern. Keep cross-crate bleed low.

## Verification

- `cargo check -p common`

## Child DOX Index

- None.
