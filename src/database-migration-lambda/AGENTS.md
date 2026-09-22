# DOX

## Purpose

- Own private manual PostgreSQL role bootstrap and root schema migration Lambda.

## Core Design

- Invoke only from `.github/workflows/initialize.yml` after protected environment approval.
- Read exact admin, runtime, migrator, and replication Secrets Manager ARNs. Never log them or their values.
- Use private VPC networking, migration security group, verified RDS TLS, one connection, and a PostgreSQL advisory lock.
- Bootstrap roles as `aura_admin`, grant its `aura_migrator` membership plus migrator database `CREATE`, then run embedded root SQLx migrations as `aura_migrator`.
- No API route, schedule, event source, business use case, or automatic CloudFormation invocation.

## Verification

- `cargo test -p database-migration-lambda --all-features`
- `cargo check -p database-migration-lambda --all-targets --all-features`
