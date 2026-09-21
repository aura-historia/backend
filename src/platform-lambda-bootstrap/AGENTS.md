# DOX

## Purpose

- Own Lambda root config and cold-start logging.
- Reuse typed PostgreSQL Lambda config. No business flow.

## Core Design

- Read typed root config only at Lambda composition root.
- Own a typed invocation budget from Lambda deadline, caller cap, and response headroom; elapsed and exhausted values saturate to zero.
- Return safe config errors. Never echo config values.
- Own version-keyed warm composition leases: one version builds once, and an old lease keeps its pool alive through an active invocation.
- It has no AWS client or credential provider adapter. Real-stage and fixture credential providers belong to `platform-postgres-secretsmanager`.

## Verification

- `cargo test -p platform-lambda-bootstrap --all-features`
