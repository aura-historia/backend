# DOX

## Purpose

- Own FxRatesApi adapter for canonical FX quote-provider port.

## Core Design

- External HTTP adapter only.
- Provider decimal rates parse to scaled integers with half-up rounding.
- Provider DTOs, including provider currency values manually mapped to canonical `money` values, token, and HTTP stay private.

## Ownership

- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo check -p fxrate-fxratesapi`
- `cargo test -p fxrate-fxratesapi --all-features`
