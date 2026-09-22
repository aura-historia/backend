# Crawler operations

This document is needed because crawler operation has separate state, credentials, migrations, review access, and a dual-database boundary.

## Runtime prerequisites

The `server` binary needs:

| Input | Purpose |
|---|---|
| `LOCAL_DB_URL` | Crawler-local Postgres connection. |
| `BUSINESS_DATABASE_URL` | Authoritative ListingSource reads and ProductListing raw capture. |
| `SPIDER_MAX_SIZE_BYTES` | Required spider page-body ceiling; use a value from 1 MiB through 8 MiB. |
| `VERTEX_AI_PROJECT_ID`, `VERTEX_AI_LOCATION` | Vertex AI configuration. |
| Application Default Credentials | Vertex AI authentication; `GOOGLE_APPLICATION_CREDENTIALS` is one local option. |

`VERTEX_AI_MODEL` selects the schema model. `CRAWLER_VERTEX_AI_CHEAP_MODEL` and operation-specific model settings can select lower-cost models. `CRAWLER_LLM_MAX_CONCURRENT_REQUESTS` and `CRAWLER_LLM_MIN_REQUEST_INTERVAL_MS` bound all crawler LLM calls.

Optional `CRAWLER_CLOUDWATCH_LOG_GROUP` and `CRAWLER_CLOUDWATCH_LOG_STREAM` enable CloudWatch log export. That deployment needs permission to create the group and stream and put log events. Logs must expose operational outcomes and identifiers without raw source payloads.

`server` and `demo` apply crawler-local migrations at startup. Migrations under [`src/crawler/migrations`](../../src/crawler/migrations/) are the authoritative crawler database contract; do not repair the live schema by hand.

## Local database

[`src/crawler/docker-compose.yml`](../../src/crawler/docker-compose.yml) starts PostgreSQL 16. The Linux and Windows scripts both create the crawler server and demo databases, migrate them, report their status, reset them, and stop the container. Keep both script sets aligned.

The local workflow requires Docker and `sqlx-cli` with PostgreSQL support. From `src/crawler`, use the matching scripts in `scripts/linux/` or `scripts/windows/`:

- `db-up` starts Postgres and creates local databases.
- `db-migrate` applies crawler migrations.
- `db-status` reports migration status.
- `db-reset` rebuilds local crawler databases.
- `db-down` stops the container.

## Review service

The server also hosts the crawler review console. It binds to `127.0.0.1:7878` unless `CRAWLER_REVIEW_BIND_ADDR` overrides it.

`CRAWLER_REVIEW_AUTH_TOKEN` protects review reads and mutations when configured, and is mandatory for a non-loopback bind. Domain registration, removal, and other mutations require bearer-token authorization. The console can exchange that token for a short-lived `HttpOnly`, `SameSite=Strict` session cookie for browser reads and iframe previews; it does not authorize ordinary mutations. A non-loopback deployment needs TLS at the serving proxy because the cookie is secure.

`GET /health` is the only unauthenticated liveness endpoint and returns no crawler state. With a review token configured, `/api/health` is authenticated. `CRAWLER_REVIEW_REQUIRED` and `CRAWLER_REVIEW_URL_PATTERN_REQUIRED` enable review gates for schemas and URL patterns.

## Validation

Run targeted checks after crawler changes:

```sh
cargo check -p crawler
cargo test -p crawler --all-features
cargo test -p crawler --tests
```

For database or local-flow changes, verify migrations, both script folders, the compose file, and affected integration tests together.
