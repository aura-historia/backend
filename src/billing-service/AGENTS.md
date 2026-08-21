# DOX

## Purpose

- Own canonical billing use cases and Stripe capability ports.

## Core Design

- Depends on `application`, `user-core`, and `user-service`; Stripe transport stays in the adapter crate.
- Operation context and boxed error contracts come from `application`.


## Ownership

- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo check -p billing-service`
- `cargo test -p billing-service --all-features`
