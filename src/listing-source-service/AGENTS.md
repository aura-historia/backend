# DOX

## Purpose

- Own ListingSource use cases and ports.

## Core Design

- Create and update own PostgreSQL transaction scope.
- Create may atomically persist a new Party and ListingSource through transaction-bound factories.
- Provider readers return focused safe models; secrets never leave verifier boundaries.
- Party-based provider/grant runtime wiring waits for Iteration 5. Existing Shop composition stays untouched.

## Verification

- `cargo check -p listing-source-service`
- `cargo test -p listing-source-service --all-features`
