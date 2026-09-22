# DOX

## Purpose

- Own narrow AWS Secrets Manager adapter for rotated PostgreSQL Lambda credentials.

## Core Design

- Reads `POSTGRES_SECRET_ARN`, or one explicitly supplied exact secret ARN, with `GetSecretValue` at `AWSCURRENT`.
- Parses only credential fields and returns opaque secret version IDs.
- Never logs, formats, or exposes secret values. No secret rotation mutation.
- Depends on the neutral PostgreSQL credential contract; no business code, pool, or handler wiring.
- Production Tokio use stays `time` only; unit tests add `macros` and `rt` for `#[tokio::test]`.

## Verification

- `cargo check -p platform-postgres-secretsmanager --all-targets --all-features`
- `cargo test --locked --manifest-path src/platform-postgres-secretsmanager/Cargo.toml --all-features -- --nocapture`
