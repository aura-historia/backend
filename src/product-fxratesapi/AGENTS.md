# DOX

## Purpose

- Own `product-fxratesapi` crate.
- Implement Product FX quote-provider port for FxRatesApi.

## Core Design

- External HTTP adapter only.
- Depends on `product-service`; no Product core/domain behavior.
- Keeps provider DTOs, token, HTTP, and response mapping private.

## Ownership

- This doc rules `src/product-fxratesapi/**`.
- Parent doc: `src/AGENTS.md`.

## Local Contracts

- Read root, `src/AGENTS.md`, then here before edit.
- Update this file when provider contract, config, or dependency changes.

## Verification

- `cargo check -p product-fxratesapi`
- `cargo test -p product-fxratesapi --all-features`
