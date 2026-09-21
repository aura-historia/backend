# DOX

## Purpose

- Own Lambda root config and cold-start logging.
- Reuse typed PostgreSQL Lambda config. No business flow.

## Core Design

- Read typed root config only at Lambda composition root.
- Return safe config errors. Never echo config values.
- Reuse pool/client handles in warm process. No request state, timer, secret store, or provider adapter.
- PostgreSQL rotation policy stays `platform-postgres` / F5.

## Verification

- `cargo test -p platform-lambda-bootstrap --all-features`
