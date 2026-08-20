# DOX

## Purpose

- Own technology-neutral shared application contracts.

## Core Design

- Own transaction lifecycle, boxed-error, pagination, patch, and personalization contracts used by several service crates.
- No entity rules, SQLx, SDK, HTTP, runtime config, or environment reads.

## Ownership

- This doc rule `src/application/**`.
- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo check -p application --all-targets --all-features`
- `cargo test -p application --all-features`
