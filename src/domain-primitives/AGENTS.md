# DOX

## Purpose

- Own domain-neutral primitives and newtype macros.

## Core Design

- Own `ChangeOutcome`, generic events, version wrappers, reusable UUID/string newtype support, slug IDs/macros, and generic query values.
- No entity IDs, business rules, transport, persistence, SDKs, or runtime config.
- `test-data` is explicit. Macro callers need their own matching feature and `fake` dependency.

## Ownership

- This doc rule `src/domain-primitives/**`.
- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo check -p domain-primitives --all-targets --all-features`
- `cargo test -p domain-primitives --all-features`
