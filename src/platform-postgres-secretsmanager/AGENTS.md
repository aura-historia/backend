# DOX

## Purpose

- Own narrow AWS Secrets Manager adapter for rotated PostgreSQL Lambda credentials.

## Core Design

- Reads one exact `POSTGRES_SECRET_ARN` with `GetSecretValue` at `AWSCURRENT`.
- Parses only credential fields and returns opaque secret version IDs.
- Never logs, formats, or exposes secret values. No secret rotation mutation.
- Depends on the neutral PostgreSQL credential contract; no business code, pool, or handler wiring.

## Verification

- `cargo check -p platform-postgres-secretsmanager --all-targets --all-features`
- `cargo test -p platform-postgres-secretsmanager --all-features`
