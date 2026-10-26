# Crawler operations

This document is needed because crawler operation has separate state, credentials, migrations, review access, and a dual-database boundary.

## Runtime prerequisites

The `server` binary needs:

| Input | Purpose |
|---|---|
| `LOCAL_DB_URL` | Crawler-local Postgres connection. |
| `BUSINESS_DATABASE_URL` | Authoritative ListingSource reads and ProductListing raw capture. |
| `SPIDER_MAX_SIZE_BYTES` | Required spider page-body ceiling; use a value from 1 MiB through 8 MiB. |
| `VERTEX_AI_PROJECT_ID`, `VERTEX_AI_LOCATION` | Vertex AI configuration; not read with `CRAWLER_AUTHORING_MODE=manual`. |
| Application Default Credentials | Vertex AI authentication; `GOOGLE_APPLICATION_CREDENTIALS` is one local option. |

`CRAWLER_AUTHORING_MODE` is the one setting that decides how URL patterns and product schemas are authored and reviewed; see [authoring modes](#authoring-modes). With `manual` the crawler makes no LLM call and needs no Vertex AI setting.

`VERTEX_AI_MODEL` selects the schema model. `CRAWLER_VERTEX_AI_CHEAP_MODEL` and operation-specific model settings can select lower-cost models. `CRAWLER_LLM_MAX_CONCURRENT_REQUESTS` and `CRAWLER_LLM_MIN_REQUEST_INTERVAL_MS` bound all crawler LLM calls.

Optional `CRAWLER_CLOUDWATCH_LOG_GROUP` and `CRAWLER_CLOUDWATCH_LOG_STREAM` enable CloudWatch log export. That deployment needs permission to create the group and stream and put log events. Logs must expose operational outcomes and identifiers without raw source payloads.

The local `dev` binary runs the same pipeline through the shared [composition root](README.md#binaries) and reads the same LLM, spider and review settings. It replaces `BUSINESS_DATABASE_URL`, AWS credentials and CloudWatch export with demo ListingSources, a raw-observation file and stdout logging, starts the local database container itself, and defaults `SPIDER_MAX_SIZE_BYTES` to 8 MiB when it is unset; `server` requires the variable.

`server` and `dev` apply crawler-local migrations at startup. Migrations under [`src/crawler/migrations`](../../src/crawler/migrations/) are the authoritative crawler database contract; do not repair the live schema by hand. This lifecycle is outside the AWS Deploy/Migrate/Initialize workflows: manual Migrate owns only business PostgreSQL migrations, and FX-only Initialize never migrates crawler state.

The crawler scheduler is deployed as a single process. Its spider and scraper workers may process many domains concurrently, while process-local per-component domain locks plus URL and ListingSource locks coordinate work inside that process. Spider and scraper may work on the same domain concurrently, each bounded by its own per-domain throttle. Different domains remain concurrent; multi-process or horizontally scaled scheduler coordination is outside the current design.

## Stopping the crawler

`server` and `dev` stop gracefully on SIGTERM (`systemctl stop`) or SIGINT (Ctrl+C). The crawler logs `crawler.shutdown.started` naming the signal, stops the review console and starts no new spider or scraper pass, spider crawl or scraper domain batch; a running scraper batch finishes its current URL and defers the rest to a later run. Running work then has a grace period, 30 seconds by default, to finish, and raw observations already scraped are flushed to raw capture before the process exits 0 with `crawler.shutdown.completed`. Work still running when the grace period ends is abandoned and the process exits 0 with `crawler.shutdown.grace_period_expired`; an abandoned spider crawl or scraper URL records no outcome and is selected again after restart. A second signal during the grace period exits at once with 128 plus the signal number. Give the service manager a stop timeout longer than the grace period (systemd's default `TimeoutStopSec` of 90 seconds is enough) so it does not escalate to SIGKILL first.

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

`GET /health` is the only unauthenticated liveness endpoint and returns no crawler state. With a review token configured, `/api/health` is authenticated.

`GET /api/listing_sources` is the operator's overview: each ListingSource with its pending reviews, URL counts and, per domain, the last crawl outcome, failure kind, failure count and next crawl time as [crawl completion](scheduling-and-retries.md#spider-crawl-completion) records them. It lists ListingSources with a domain waiting for an operator first, so a domain waiting for a review or for an operator is visible there without the logs.

### Authoring modes

`CRAWLER_AUTHORING_MODE` decides, for URL patterns and product schemas alike, who authors them and whether they wait for human review:

| Value | URL patterns | Product schemas |
|---|---|---|
| `auto` | Inferred by the LLM and applied directly through an approved review. | Generated by the LLM; approved without review when deterministic coverage and every [plausibility check](extraction-and-review.md#plausibility-checks) pass and the LLM evaluation is a high-confidence approval, otherwise pending review. |
| `review` (default) | Inferred by the LLM, pending review. | Generated by the LLM, pending review. |
| `manual` | No LLM; see below. | No LLM; see below. |

Every mode records artifacts as reviews, so the console always shows what became active and why; a pending review for a domain or ListingSource blocks new generation in every mode. A pending product-schema review does not stop scraping in any mode: pages a cached schema extracts are scraped as usual, and only a page that fails every cached schema waits on the review with a [growing retry delay](scheduling-and-retries.md#fetch-retries) and is [attached to it](extraction-and-review.md#review-rail). A ListingSource without cached schemas waits on its initial review entirely. Approval releases the waiting pages for an immediate retry. The LLM confidence evaluation is recorded on every generated schema review. The default is `review`, so auto-approval is always an explicit opt-in. An empty or whitespace-only value (a `NAME=` placeholder) counts as unset; any other value fails startup with an error naming the variable, the rejected value and the accepted values. The crawler logs the effective mode, whether it came from the environment or the default, and what the mode does, once at startup.

`CRAWLER_REVIEW_REQUIRED`, `CRAWLER_REVIEW_URL_PATTERN_REQUIRED` and `CRAWLER_SCHEMA_LLM_REVIEW_MODE` are removed; a non-empty value in any of them fails startup so a dead setting cannot be mistaken for an active one. Their combinations map to the new modes as follows: review required with `auto_approve_high_confidence` → `auto`; review required with `human_only` or `report_only` → `review`. Running without review gates has no equivalent: `auto` is the closest mode, but unlike the old setting it holds a generated schema for review when the LLM evaluation is not a high-confidence approval, so its review queue needs an operator.

### Manual authoring

With `CRAWLER_AUTHORING_MODE=manual`, where the LLM would run the crawler opens a pending review instead, or joins the one already pending, and the affected work waits until an operator approves it:

| Review reason | Opened when | Candidate | Operator action |
|---|---|---|---|
| `manual_url_pattern_required` | A domain needs a product URL pattern | Decision `UNDECIDED` with the crawled URL sample | Enter a regex or `NO_PATTERN` as the candidate, then approve. An undecided candidate cannot be approved. The crawl that opened the review finishes discovery and stores its URLs; approval classifies them with the decision and releases the domain ([crawl completion](scheduling-and-retries.md#spider-crawl-completion)); a running crawl that has not classified yet uses the pattern too, without opening another review. |
| `manual_schema_required` | A ListingSource has no product schema, or its cached schemas stop matching a page | The cached schemas plus a generic starter schema, with the seed or triggering page | Rewrite the starter schema, check it with the live matrix, then approve. Approval replaces the ListingSource's schemas with the reviewed list. For a sold or withdrawn item the shop serves at HTTP 200 with a removal notice and no product markup, [declare the page removed](extraction-and-review.md#removed-page-templates) on the same review instead of authoring a schema for it; rejecting the review would classify it as a non-product page and leave its listing visible. |

A domain the spider records as `NEEDS_OPERATOR` ([repeated deterministic failures](scheduling-and-retries.md#scraper-domain-circuits)) opens no review: the console lists it above the review queue with its last failure kind and failure count, and the operator fixes the configuration (the right crawl root, or removing and re-registering the domain) or disables crawling for the ListingSource. A `NO_PATTERN` decision that turns out wrong is reset the same way.

A page that is not a product page should be excluded by narrowing the URL pattern. Rejecting a `manual_schema_required` review classifies its seed and triggering pages as non-product pages, so the review is not reopened for them; its waiting and reviewer-added pages keep their class. To keep a candidate that is not ready yet, give the review the needs-repair [verdict](extraction-and-review.md#review-rail) instead of rejecting it. A product page that leaves its title or state empty where a cached schema finds the element opens no review; it records a [missing required value](extraction-and-review.md#missing-required-values) instead, so an untitled product is never rejected as a non-product page.

## Validation

Run targeted checks after crawler changes:

```sh
cargo check -p crawler
cargo test -p crawler --all-features
cargo test -p crawler --tests
```

For database or local-flow changes, verify migrations, both script folders, the compose file, and affected integration tests together.
