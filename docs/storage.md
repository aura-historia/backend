# Storage ownership

This repo is migrating business truth from DynamoDB to Postgres. Public API behavior must stay stable during the migration.

## Target owners

Postgres owns business truth for:

- users
- access-token canonical metadata
- shops
- partner-shop-applications
- products
- product-events
- product-watchlist
- search-filters
- search-filter matches

DynamoDB keeps:

- notifications with TTL and insert-to-send behavior
- OAuth short-lived authorization/exchange codes
- FX rate
- required auth/token lookup cache projected from Postgres

OpenSearch keeps search projections only. It is rebuildable.

## Postgres runtime contract

Postgres is self-hosted. No RDS Proxy.

Infra provides these env vars to explicitly opted-in Lambdas and service tasks:

- `POSTGRES_HOST`
- `POSTGRES_PORT`
- `POSTGRES_DATABASE`
- `POSTGRES_USERNAME`
- `POSTGRES_PASSWORD`
- `POSTGRES_MAX_CONNECTIONS`

Real stages resolve host/user/db/password from SSM. Ephemeral uses local Docker defaults for integration tests.

Lambda crates should build pools with `common::postgres::PostgresPoolConfig` and keep pools small. Default max is `2`.

## Migration location

Each business crate owns its SQLx migrations under:

```text
src/<crate>/migrations/
```

Use SQLx migration filenames with sortable prefixes:

```text
YYYYMMDDHHMMSS_short_description.sql
```

Migration files must be idempotent where integration tests re-run them (`CREATE TABLE IF NOT EXISTS`, `CREATE INDEX IF NOT EXISTS`). Keep table names owned by the crate unless a documented cross-crate relation exists.

## Repository pattern

For a Postgres-backed entity:

- Keep the domain trait clean in the crate service/domain seam.
- Put SQL implementation under `src/<crate>/src/postgres/`.
- Name SQL DTOs `Row` when they represent DB rows.
- Handlers depend on service traits, not SQLx.
- Services may depend on repository traits, not concrete pool construction.
- Use typed errors with `thiserror`; map `sqlx::Error` at the repository edge.
- No global outbox table. Outbox tables are owner/action/entity-specific.

Suggested shape:

```text
src/<crate>/src/postgres/mod.rs
src/<crate>/src/postgres/repository.rs
src/<crate>/src/postgres/<entity>_row.rs
src/<crate>/migrations/...
```

## Test pattern

Use `test-api` for repository integration tests:

```rust
use test_api::*;

const POSTGRES: Postgres =
    Postgres::new("src/<crate>/migrations");

#[aura_integration_test(services = [POSTGRES])]
async fn should_persist_entity_when_valid() {
    let pool = get_postgres_client().await;
    // build repository with &pool
}
```

If seed data is needed:

```rust
const POSTGRES: Postgres = Postgres::with_setup_script(
    "src/<crate>/migrations",
    "src/<crate>/tests/fixtures/setup.sql",
);
```

The test harness starts one Postgres Docker container per test binary, reruns migrations before each test, and truncates public tables after each test.
