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

The local `dev` binary runs the same pipeline through the shared [composition root](README.md#binaries) and reads the same LLM, spider and review settings. It replaces `BUSINESS_DATABASE_URL`, AWS credentials and CloudWatch export with demo ListingSources, a raw-observation file and stdout logging, and starts the local database container itself.

`server` and `dev` apply crawler-local migrations at startup. Migrations under [`src/crawler/migrations`](../../src/crawler/migrations/) are the authoritative crawler database contract; do not repair the live schema by hand. This lifecycle is outside the AWS Deploy/Migrate/Initialize workflows: manual Migrate owns only business PostgreSQL migrations, and FX-only Initialize never migrates crawler state.

The crawler scheduler is deployed as a single process. Its spider and scraper workers may process many domains concurrently, while process-local per-component domain locks plus URL and ListingSource locks coordinate work inside that process. Spider and scraper may work on the same domain concurrently, each bounded by its own per-domain throttle. Different domains remain concurrent; multi-process or horizontally scaled scheduler coordination is outside the current design.

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

`CRAWLER_REVIEW_AUTH_TOKEN` protects review reads and mutations when configured, and is mandatory for a non-loopback bind. Domain registration, removal, and other mutations require bearer-token authorization. The local `dev` binary is the only exception: on a loopback bind without a configured token it accepts mutations from same-origin browser requests, identified by an `Origin` that matches a loopback `Host`. The console can exchange that token for a short-lived `HttpOnly`, `SameSite=Strict` session cookie for browser reads and iframe previews; it does not authorize ordinary mutations. A non-loopback deployment needs TLS at the serving proxy because the cookie is secure.

`GET /health` is the only unauthenticated liveness endpoint and returns no crawler state. With a review token configured, `/api/health` is authenticated. `CRAWLER_REVIEW_REQUIRED` and `CRAWLER_REVIEW_URL_PATTERN_REQUIRED` enable review gates for schemas and URL patterns.

`CRAWLER_SCHEMA_LLM_REVIEW_MODE` decides how generated schemas pass the schema review gate:

| Value | Effect |
|---|---|
| `human_only` (default) | Every generated schema waits for human review. |
| `report_only` | Same gate as `human_only`; the name marks that the LLM evaluation is only informational. |
| `auto_approve_high_confidence` | Approves a schema without human review when deterministic coverage passes and the LLM evaluation is a high-confidence approval. |

The LLM confidence evaluation is recorded on the schema review in every mode.

The default is `human_only` whether or not review is required, so auto-approval is always an explicit opt-in. The mode only takes effect while `CRAWLER_REVIEW_REQUIRED` is enabled; without it, generated schemas are persisted directly. Any other value, including an empty one, fails startup with an error naming the variable, the rejected value and the accepted values. The crawler logs the effective review gates and whether the mode came from the environment or the default once at startup.

## Validation

Run targeted checks after crawler changes:

```sh
cargo check -p crawler
cargo test -p crawler --all-features
cargo test -p crawler --tests
```

For database or local-flow changes, verify migrations, both script folders, the compose file, and affected integration tests together.
