# DOX

## Purpose

- Own pure reusable money domain values.

## Core Design

- `Currency`, minor-unit rules, `MonetaryAmount`, and `Price` live here. `Currency::as_str` and exact `from_code` own canonical ISO currency codes.
- No FX provider/rate behavior, DTO, record, document, SQL, HTTP, AWS, or environment code.

## Ownership

- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo check -p money --all-targets --all-features`
- `cargo test -p money --all-features`
