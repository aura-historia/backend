# DOX

## Purpose

- Own Auction use cases, storage ports, and administrator authorization.

## Core Design

- Service owns Auction write transactions. `auction-core` owns facts.
- Admin create/update/get require administrator authorization. Repositories persist Auction aggregates; details readers return service views.

## Verification

- `cargo check -p auction-service --all-targets --all-features`
- `cargo test -p auction-service --all-features`
