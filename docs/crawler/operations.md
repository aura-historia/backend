# Crawler operations

This document is needed because crawler operation has separate state, credentials, migrations, review access, and a dual-database boundary.

## Runtime prerequisites

The `server` binary needs:

| Input | Purpose |
|---|---|
| `LOCAL_DB_URL` | Required connection URL of the crawler-local Postgres database. The operator creates the database; `server` only migrates it. Unset or blank fails startup; see [single-server deployment](#single-server-deployment). |
| `BUSINESS_DATABASE_URL` | Authoritative ListingSource reads and ProductListing raw capture. |
| `SPIDER_MAX_SIZE_BYTES` | Required spider page-body ceiling; use a value from 1 MiB through 8 MiB. |
| `VERTEX_AI_PROJECT_ID`, `VERTEX_AI_LOCATION` | Vertex AI configuration; not read with `CRAWLER_AUTHORING_MODE` `manual` or `agent`. |
| Application Default Credentials | Vertex AI authentication; `GOOGLE_APPLICATION_CREDENTIALS` is one local option. |

`CRAWLER_AUTHORING_MODE` is the one setting that decides how URL patterns and product schemas are authored and reviewed; see [authoring modes](#authoring-modes). With `manual` or `agent` the crawler makes no LLM call and needs no Vertex AI setting.

`VERTEX_AI_MODEL` selects the schema model. `CRAWLER_VERTEX_AI_CHEAP_MODEL` and operation-specific model settings can select lower-cost models. `CRAWLER_LLM_MAX_CONCURRENT_REQUESTS` and `CRAWLER_LLM_MIN_REQUEST_INTERVAL_MS` bound all crawler LLM calls.

Optional `CRAWLER_CLOUDWATCH_LOG_GROUP` and `CRAWLER_CLOUDWATCH_LOG_STREAM` enable CloudWatch log export. That deployment needs permission to create the group and stream and put log events. Logs must expose operational outcomes and identifiers without raw source payloads.

The local `dev` binary runs the same pipeline through the shared [composition root](README.md#binaries) and reads the same LLM, spider and review settings. It replaces `BUSINESS_DATABASE_URL`, AWS credentials and CloudWatch export with demo ListingSources, a raw-observation file and stdout logging, starts the [local database](#local-database) itself and uses its `crawler_demo` database instead of `LOCAL_DB_URL`, and defaults `SPIDER_MAX_SIZE_BYTES` to 8 MiB when it is unset; `server` requires the variable and its spider rejects each crawl without it.

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

To run `server` against this container, start it with `db-up` and set `LOCAL_DB_URL=postgres://postgres:postgres@localhost:5432/crawler_server`; `server` never starts Docker itself.

## Review service

The server also hosts the crawler review console. It binds to `127.0.0.1:7878` unless `CRAWLER_REVIEW_BIND_ADDR` overrides it.

`CRAWLER_REVIEW_AUTH_TOKEN` protects review reads and mutations when configured, and is mandatory for a non-loopback bind and for `server` in `agent` mode. Domain registration, removal, and other mutations require bearer-token authorization. The local `dev` binary is the only exception: on a loopback bind without a configured token it accepts mutations from same-origin browser requests, identified by an `Origin` that matches a loopback `Host`. The console can exchange that token for a short-lived `HttpOnly`, `SameSite=Strict` session cookie for browser reads and iframe previews; it does not authorize ordinary mutations. An API request whose `Authorization` header matches no configured token is refused with `401`, even with a valid session cookie or on the tokenless local console. A non-loopback deployment needs TLS at the serving proxy because the cookie is secure.

A human restores what an approval replaced with `POST /api/reviews/{id}/restore` (optional `notes`) or the console's **Restore previous** action on the approved review: the ListingSource's schema list and removed-page templates, or the domain's URL-pattern decision, come back as the approval's event recorded them, and the restore re-queues work as the approval did: pages waiting on a product-schema review are retried at once, and a restored URL-pattern decision classifies the domain's stored URLs again. Any approval whose event keeps what it made live can be restored, whether a human, an agent or the crawler approved it, once, and only while that configuration is still live: after a newer approval or a live edit the restore answers `409` and changes nothing, so a newer change is never overwritten. Restoring an approval that replaced no schema list leaves the ListingSource without schemas, so its next scraped page opens a new review. An agent's restore answers `403` with `reason` `RESTORE_NEEDS_HUMAN`. A restore is recorded as a `RESTORED` event; captures made while the approved artifact was live stay until the pages' next scrape.

`GET /health` is the only unauthenticated liveness endpoint and returns no crawler state. With a review token configured, `/api/health` is authenticated.

`GET /api/listing_sources` is the operator's overview: each ListingSource with its pending reviews, URL counts and, per domain, the last crawl outcome, failure kind, failure count and next crawl time as [crawl completion](scheduling-and-retries.md#spider-crawl-completion) records them. It lists ListingSources with a domain waiting for an operator first, so a domain waiting for a review or for an operator is visible there without the logs. `GET /api/reviews`, the console's queue, lists pending reviews of crawl-enabled ListingSources first, then pending reviews of disabled ones, which the console marks and which hold back no crawl or scrape, then closed reviews, newest first within each.

### Authoring modes

`CRAWLER_AUTHORING_MODE` decides, for URL patterns and product schemas alike, who authors them and whether they wait for human review:

| Value | URL patterns | Product schemas |
|---|---|---|
| `auto` | Inferred by the LLM and applied directly through an approved review. | Generated by the LLM; approved without review when deterministic coverage and every [plausibility check](extraction-and-review.md#plausibility-checks) pass and the LLM evaluation is a high-confidence approval, otherwise pending review. |
| `review` (default) | Inferred by the LLM, pending review. | Generated by the LLM, pending review. |
| `manual` | No LLM; see [manual authoring](#manual-authoring). | No LLM; see [manual authoring](#manual-authoring). |
| `agent` | As `manual`, without keyword classes; the author agent labels the URL sample and stages the pattern, and the reviewer agent approves or rejects it within the approval guard ([review agents](#review-agents)). | As `manual`; the author agent stages the schemas and the reviewer agent approves or rejects them within the approval guard. |

Every mode records artifacts as reviews, so the console always shows what became active and why; a pending review for a domain or ListingSource blocks new generation in every mode. A pending product-schema review does not stop scraping in any mode: pages a cached schema extracts are scraped as usual, and only a page that fails every cached schema waits on the review with a [growing retry delay](scheduling-and-retries.md#fetch-retries) and is [attached to it](extraction-and-review.md#review-rail). A ListingSource without cached schemas waits on its initial review entirely. Approving or rejecting the review releases the waiting pages for an immediate retry. The LLM confidence evaluation is recorded on every generated schema review. The default is `review`, so auto-approval is always an explicit opt-in. An empty or whitespace-only value (a `NAME=` placeholder) counts as unset; any other value fails startup with an error naming the variable, the rejected value and the accepted values. The crawler logs the effective mode, whether it came from the environment or the default, and what the mode does, once at startup.

`CRAWLER_REVIEW_REQUIRED`, `CRAWLER_REVIEW_URL_PATTERN_REQUIRED` and `CRAWLER_SCHEMA_LLM_REVIEW_MODE` are removed; a non-empty value in any of them fails startup so a dead setting cannot be mistaken for an active one. Their combinations map to the new modes as follows: review required with `auto_approve_high_confidence` → `auto`; review required with `human_only` or `report_only` → `review`. Running without review gates has no equivalent: `auto` is the closest mode, but unlike the old setting it holds a generated schema for review when the LLM evaluation is not a high-confidence approval, so its review queue needs an operator.

### Manual authoring

With `CRAWLER_AUTHORING_MODE` `manual` or `agent`, where the LLM would run the crawler opens a pending review instead, or joins the one already pending, and the affected work waits until an operator, or in `agent` mode the [review agents](#review-agents), approve it. Persisted review reasons are the same in both modes:

| Review reason | Opened when | Candidate | Operator action |
|---|---|---|---|
| `manual_url_pattern_required` | A domain needs a product URL pattern | Decision `UNDECIDED` with the crawled URL sample | Enter a regex or `NO_PATTERN` as the candidate, then approve. An undecided candidate cannot be approved. The crawl that opened the review finishes discovery and stores its URLs; approval classifies them with the decision and releases the domain ([crawl completion](scheduling-and-retries.md#spider-crawl-completion)); a running crawl that has not classified yet uses the pattern too, without opening another review. |
| `manual_schema_required` | A ListingSource has no product schema, or its cached schemas stop matching a page | The cached schemas plus a generic starter schema, with the seed or triggering page | Rewrite the starter schema, check it with the live matrix, then approve. Approval replaces the ListingSource's schemas with the reviewed list. For a sold or withdrawn item the shop serves at HTTP 200 with a removal notice and no product markup, [declare the page removed](extraction-and-review.md#removed-page-templates) on the same review instead of authoring a schema for it; rejecting the review would classify it as a non-product page and leave its listing visible. |

A domain the spider records as `NEEDS_OPERATOR` ([repeated deterministic failures](scheduling-and-retries.md#scraper-domain-circuits)) opens no review: the console lists it above the review queue with its last failure kind and failure count, and the operator fixes the configuration (the right crawl root, or removing and re-registering the domain) or disables crawling for the ListingSource. A `NO_PATTERN` decision that turns out wrong is reset the same way.

A page that is not a product page should be excluded by narrowing the URL pattern. Rejecting a `manual_schema_required` review classifies its seed and triggering pages as non-product pages, so the review is not reopened for them; its waiting and reviewer-added pages keep their class. To keep a candidate that is not ready yet, give the review the needs-repair [verdict](extraction-and-review.md#review-rail) instead of rejecting it. A product page that leaves its title or state empty where a cached schema finds the element opens no review; it records a [missing required value](extraction-and-review.md#missing-required-values) instead, so an untitled product is never rejected as a non-product page.

### Review agents

With `CRAWLER_AUTHORING_MODE=agent` the crawler works as with `manual`, and two review agents work its reviews through the review console as an operator does, each with a credential of its own: the author agent stages URL patterns and product schemas, the reviewer agent approves or rejects them. The [event trail](extraction-and-review.md#review-rail) records them as the actors `AGENT_AUTHOR` and `AGENT_REVIEWER`.

In `agent` mode the crawler does not guess what a product page is; Claude decides. The author agent labels URLs of a URL-pattern review's domain, its sample's among them, `PRODUCT` or `NOT_PRODUCT` with a reason and stages a pattern, and the reviewer agent's approval is taken only when the pattern follows those [sample labels](extraction-and-review.md#approval-guard). The spider, a URL-pattern review's URL sample and the classification that approving or restoring a pattern applies classify a URL by the domain's approved pattern alone: a URL the pattern takes is `product`, every other URL stays `other`, unclassified. The keyword classes `category`, `imprint` and `info` that the other modes derive from words in a URL, its host included, are never assigned. Until the pattern is approved every crawled URL stays unclassified, so nothing is scraped yet; the crawl stores its URLs and waits on the URL-pattern review it opens with its URL sample, as in [manual authoring](#manual-authoring), and approval classifies the stored URLs with the pattern.

| Request | Author agent | Reviewer agent |
|---|---|---|
| Console API reads, including a product-schema review's live matrix (`GET /api/reviews/{id}/matrix?refresh=true`), a page's projection (`GET /api/reviews/{id}/page-projection`) and the evaluation of a proposed candidate (`POST /api/reviews/{id}/evaluate`), which store nothing ([review rail](extraction-and-review.md#review-rail)) | Yes | Yes |
| Live previews of any URL (`GET /api/live-inspect`, `/api/live-html`) | Only with `review` naming a review and `url` on its ListingSource's domains under the review-page rule, otherwise `400` | As the author agent |
| Candidate and schema-field edits, removed-page declarations, sample labels (`POST /api/reviews/{id}/candidate`, `/schema-field`, `/sample-labels`), review page additions and removals (`POST /api/reviews/{id}/pages`, `DELETE /api/review-pages/{id}`) | Yes | `403` `REVIEWER_CANNOT_CHANGE_CANDIDATES` |
| Needs repair with its notes (`POST /api/reviews/{id}/needs-repair`) | Yes | Yes |
| Approve and reject (`POST /api/reviews/{id}/approve`, `/reject`) | `403` `AUTHOR_CANNOT_DECIDE` | Within the approval guard; a rejection needs non-blank `notes` (`400`) |
| Restore (`POST /api/reviews/{id}/restore`) | `403` `RESTORE_NEEDS_HUMAN` | `403` `RESTORE_NEEDS_HUMAN` |
| Browser sessions (`POST /api/session`, `/api/session/logout`), domain registration and removal | `401` | `401` |

Nobody configures the agents' tokens. At every start in `agent` mode, `server` and `dev` generate a fresh 256-bit token per role from the operating system's CSPRNG and write both to `review-agent-tokens.json` in the crawler's working directory, its state directory (`/var/lib/crawler` under the [systemd unit](#systemd-unit)): `{"author": "<64 hex characters>", "reviewer": "<64 hex characters>"}`. The file is written under a temporary name with mode `0600` and renamed into place, so a reader never sees a partial file and only the crawler's user can read it; the [review MCP server](#review-mcp-server) runs as that user and reads its role's token from it. A start that cannot write the file fails. The console compares a bearer with every token in constant time. Tokens are never logged or printed: the startup log names the file's path, and `Crawler review console listening` logs `agent_access`. A restart replaces both tokens and so revokes the old ones. In every other mode no agent acts: the crawler writes no token file and removes one an earlier start left. `CRAWLER_REVIEW_AGENT_TOKEN` and `CRAWLER_REVIEW_AGENT_APPROVALS` are no settings; a value in either fails startup.

The agents always work next to a human. `server` in `agent` mode fails startup without `CRAWLER_REVIEW_AUTH_TOKEN`, so a human can reject, restore and override what the agents do. In `dev` the tokenless loopback console stands in for it: same-origin requests without a token keep acting as `HUMAN` while the agents send their tokens. A request without an agent's token never acts as an agent.

Agents change pending reviews only: their candidate and field edits, removed-page declarations, sample labels and page additions and removals on any other review, including the approved review whose candidate is live, answer `403` `AGENT_CHANGES_ONLY_PENDING_REVIEWS` and record nothing, so they change the live configuration only through an approval. The reviewer agent's approval is taken only when the [approval guard](extraction-and-review.md#approval-guard) passes, and one that carries `acknowledged_coverage` answers `403` `AGENT_COVERAGE_ACKNOWLEDGEMENT`. No agent approves its own work: an agent's approval is refused with `409` and `failed_checks` `[{"check": "APPROVER_IS_AUTHOR"}]` when the event trail shows that the approving agent changed the candidate version it approves or the review's pages; humans approve as before. `CRAWLER_REVIEW_AGENT_MAX_DECISIONS_PER_HOUR` (default 20) caps the agents' approvals plus rejections within any rolling hour, counted from the event trail so a restart does not reset it; above it the API answers `429` with a `Retry-After` header and `retry_after_secs`. `CRAWLER_REVIEW_AGENT_MATRIX_MAX_AGE_MINUTES` (default 30) bounds how long ago the live matrix an agent approval of product schemas rests on fetched its pages; sample labels do not expire. `CRAWLER_REVIEW_AGENT_MIN_LABELLED_URLS` (default 3) is how many URLs an agent approval of a URL pattern needs labelled `PRODUCT`, and how many `NOT_PRODUCT`. Empty or whitespace-only values count as unset; any other value that is not a whole number of at least 1 fails startup with the variable and the value. `Crawler review console listening` logs `agent_access`, `agent_approvals` (`guarded` in `agent` mode, `off` otherwise), `agent_max_decisions_per_hour`, `agent_matrix_max_age_minutes` and `agent_min_labelled_urls`.

The kill switch needs no deploy: set `CRAWLER_AUTHORING_MODE=manual` and restart. The restart removes the token file, the agents' tokens answer `401`, and humans work the reviews as in [manual authoring](#manual-authoring); a human [restores](#review-service) any agent approval that should not stay live.

### Review MCP server

The agents reach the console only through `review-mcp`, an MCP server over stdio that the agents' MCP client starts once per agent process, in exactly one role: `review-mcp --role author` or `review-mcp --role reviewer`. Build it with `cargo build --release --locked -p crawler --bin review-mcp` next to `server` and run it as the crawler's user, the only one who can read the token file. It is a thin client of the console API: it has no database access and no review rule of its own, so the console's role refusals, [approval guard](extraction-and-review.md#approval-guard), decision cap and URL rules apply to every tool call.

| Setting | Meaning |
|---|---|
| `CRAWLER_REVIEW_CONSOLE_URL` | The console's base URL, default `http://127.0.0.1:7878`. Plain http is accepted for a loopback host only; credentials, a query or a fragment fail startup. |
| `CRAWLER_REVIEW_AGENT_TOKEN_FILE` | The token file, default `review-agent-tokens.json` in the working directory. Point it at the crawler's state directory, `/var/lib/crawler/review-agent-tokens.json` under the [systemd unit](#systemd-unit). |
| `LOG_LEVEL` | Level of its JSON log on stderr, default `info`. |

An empty or whitespace-only setting counts as unset. A missing or unknown `--role`, or any other argument, fails startup with exit code 2. The role's token comes from the token file only, never from a setting, an argument or a tool call: `review-mcp` reads it at startup, and fails when it cannot, as outside `agent` mode, and reads it again when the console answers `401`, retrying the request once when a crawler restart has rotated the token. It connects to the console directly, ignoring proxy settings, and follows no redirect, so the token reaches only the configured console. stdout carries the MCP session only; the log names each tool call with its outcome and duration, never its arguments, its answer, a page or a token.

A role's server lists and runs only its own tools; the tool descriptions in [`review_mcp/tools`](../../src/crawler/src/review_mcp/tools) are the agents' contract.

| Role | Tools |
|---|---|
| Both | `list_pending_reviews`, `get_review`, `inspect_page`, `refresh_matrix`, `mark_needs_repair` |
| Author | `evaluate_candidate`, `update_candidate`, `update_schema_field`, `add_review_page`, `remove_review_page`, `declare_removed_page`, `label_sample_urls` |
| Reviewer | `approve`, `reject` |

A console refusal comes back as a tool error whose text is a JSON object with `kind`, `error`, `status` and the console's own `reason`, `failed_checks`, `candidate_version`, `coverage`, `retry_after_secs`, `limit` and `details`. A tool taking a URL refuses one that is not a plain http(s) URL on a domain of the review's ListingSource (`URL_OUTSIDE_REVIEW_SOURCE`) before any request, and the console refuses it again. No answer carries a token, cookie or header. Every answer stays below 48 KiB of JSON, so it fits the 50,000 characters Claude Code keeps of a tool result in the agent's context (a longer one is replaced by a preview of a file the agents cannot read): long texts end in a `[truncated: ...]` marker, `inspect_page` answers at most 36,000 characters of a page's projection (fewer for text in non-Latin scripts), with a marker line where it stops and an `offset` to read on from there, and each shortened list is named in the answer's `truncations`. `refresh_matrix` and `evaluate_candidate` wait for the console's background work for up to 15 minutes, then answer `STILL_RUNNING`.

For the Claude Code CLI, one role's MCP configuration reads:

```json
{"mcpServers": {"crawler_review": {"command": "/opt/crawler/bin/review-mcp", "args": ["--role", "author"],
  "env": {"CRAWLER_REVIEW_AGENT_TOKEN_FILE": "/var/lib/crawler/review-agent-tokens.json"}}}}
```

## Single-server deployment

Production runs one `server` process on one Linux host under systemd. Start from the example unit [`crawler-server.service`](../../src/crawler/scripts/linux/crawler-server.service) and environment file [`.env.example`](../../src/crawler/.env.example); real values live only on the host.

### Build and host setup

Build with `cargo build --release --locked -p crawler --bin server` and copy `target/release/server` to the host, for example `/opt/crawler/bin/server`. Crawler-local migrations are embedded in the binary.

`server` connects to the database in `LOCAL_DB_URL` and applies pending crawler-local migrations; it never starts Docker or creates the database. Provide PostgreSQL 16, as in local development, with its contrib modules, on the host or reachable from it. Create a dedicated role that owns the database, so migrations can create their tables and the trusted `pgcrypto` extension without superuser rights:

```sql
CREATE ROLE crawler LOGIN PASSWORD '<random password>';
CREATE DATABASE crawler_server OWNER crawler;
```

Set `LOCAL_DB_URL=postgres://crawler:<password>@127.0.0.1:5432/crawler_server` in the environment file; percent-encode reserved characters in the password. A missing or blank value fails startup with `missing required environment variable: LOCAL_DB_URL`. Keep the Postgres port closed to the outside. The database holds all crawler state; back it up.

`BUSINESS_DATABASE_URL` must reach the business PostgreSQL from the host, for example through a security-group rule for the host's address and `sslmode=require` in the URL. A failed connection at startup stops the process. Business migrations belong to the manual Migrate workflow, never to the crawler, so the business schema must already be current.

Prepare the host once:

```sh
useradd --system --home-dir /var/lib/crawler --shell /usr/sbin/nologin crawler
install -d -m 0750 -o root -g crawler /etc/crawler
install -m 0640 -o root -g crawler src/crawler/.env.example /etc/crawler/crawler.env  # then fill in
install -m 0644 src/crawler/scripts/linux/crawler-server.service /etc/systemd/system/
systemctl daemon-reload && systemctl enable --now crawler-server
```

### Authoring mode

Launch with `CRAWLER_AUTHORING_MODE=manual`, or `agent` once the [review agents](#review-agents) run on the host: neither makes an LLM call or reads a Vertex AI setting ([`config.rs`](../../src/crawler/src/runtime/config.rs)). A later switch to `review` or `auto` makes `VERTEX_AI_PROJECT_ID`, `VERTEX_AI_LOCATION` and Application Default Credentials on the host required; startup fails without them.

### Review console access

[`ReviewServerConfig`](../../src/crawler/src/review/server.rs) applies these rules:

- `CRAWLER_REVIEW_BIND_ADDR` defaults to `127.0.0.1:7878`; any value that is not `host:port`, including an empty one, fails startup.
- An empty or whitespace-only `CRAWLER_REVIEW_AUTH_TOKEN` counts as unset. A non-loopback bind without a token fails startup with `CRAWLER_REVIEW_AUTH_TOKEN is required for non-loopback bind addresses`.
- With `CRAWLER_AUTHORING_MODE=agent`, a missing `CRAWLER_REVIEW_AUTH_TOKEN` fails startup with `CRAWLER_AUTHORING_MODE=agent needs CRAWLER_REVIEW_AUTH_TOKEN, ...`, and the `crawler` user must be able to write the working directory, which `StateDirectory=crawler` provides, for the agents' token file.
- A value in `CRAWLER_REVIEW_AGENT_TOKEN` or `CRAWLER_REVIEW_AGENT_APPROVALS`, or a decision cap, matrix age or minimum of labelled URLs that is not a whole number of at least 1, fails startup.
- `server` accepts review mutations only with the bearer token, so approving manual reviews needs the token even on loopback. Without a token, reads are open to anyone who reaches the port.

For launch, keep the loopback bind, set a random token, and tunnel: `ssh -L 7878:127.0.0.1:7878 <host>`, then open `http://127.0.0.1:7878`. The alternative is a non-loopback bind behind a TLS-terminating reverse proxy; the token is then mandatory and the session cookie is `Secure`, so browsers must use HTTPS.

### Logging

Logs are JSON on stdout, stored by the journal (`journalctl -u crawler-server`). `LOG_LEVEL` sets the base filter level (default `info`) in [`server.rs`](../../src/crawler/src/bin/server.rs); `RUST_LOG` is not read.

Setting `CRAWLER_CLOUDWATCH_LOG_GROUP` adds CloudWatch Logs export, which needs `logs:CreateLogGroup`, `logs:CreateLogStream` and `logs:PutLogEvents`; `server` creates the group and stream at every start and fails startup if that fails or either name is blank ([`logging.rs`](../../src/crawler/src/logging.rs)). Set `CRAWLER_CLOUDWATCH_LOG_STREAM`: its fallback, `HOSTNAME`, is not set by systemd, so the stream would be `unknown-host`. Credentials and region come from the AWS SDK default chain: an EC2 instance profile, a profile in the service user's `~/.aws` selected with `AWS_PROFILE` (including `credential_process`, for example IAM Roles Anywhere off AWS), or access keys in the environment file. Set `AWS_REGION` unless the profile or instance provides it.

### systemd unit

The example unit runs `server` as the unprivileged `crawler` user with `EnvironmentFile=/etc/crawler/crawler.env`, `Restart=on-failure`, `KillSignal=SIGTERM` and `TimeoutStopSec=45s`, a margin above the 30-second graceful-shutdown period before systemd sends SIGKILL. Its working directory is the empty state directory because `server` also loads a `.env` from its working directory or a parent; variables already in the environment take precedence. It starts after `network-online.target` and after `postgresql.service` when Postgres runs on the same host. A database that is still unreachable at start fails startup, and `Restart=on-failure` retries.

Run exactly one instance: no second host and no manual `server` next to the unit. The scheduler coordinates only through process-local locks ([scheduling and retries](scheduling-and-retries.md)), so a second process would crawl and capture the same work concurrently.

### Upgrade

Run `systemctl stop crawler-server`, replace the binary and run `systemctl start crawler-server`. Pending crawler-local migrations apply at startup; never repair the schema by hand. Startup refuses a database with an applied migration the binary does not contain or whose checksum changed, so rolling back to an older binary after a migration requires restoring the database.

### After start

- The journal shows `Connected to crawler-local Postgres`, `Connected to authoritative business Postgres`, `Crawler authoring mode configured: ...` with `authoring_mode` `manual` or `agent` and `authoring_mode_source` `env`, in `agent` mode `Generated the review agents' tokens of this start and wrote them to their token file` with the file's `path`, and `Crawler review console listening`.
- `curl -fsS http://127.0.0.1:7878/health` returns `{"ok":true}`.
- `curl -fsS -H "Authorization: Bearer $TOKEN" http://127.0.0.1:7878/api/listing_sources` lists the ListingSources, with domains waiting for an operator first.
- The first spider pass does not log a `SPIDER_MAX_SIZE_BYTES` error: the spider checks that value, from 1048576 through 8388608, on each crawl, not at startup.

## Metrics and alarms

The crawler publishes metrics as CloudWatch [Embedded Metric Format](https://docs.aws.amazon.com/AmazonCloudWatch/latest/monitoring/CloudWatch_Embedded_Metric_Format_Specification.html) records in its JSON log: each record is one log line whose top-level `_aws` block names the metrics, so the [CloudWatch log export](#runtime-prerequisites) turns them into metrics without a metrics client or extra permission. Without log export the records stay on stdout. Metric records ignore `LOG_LEVEL`, so a quiet log level cannot read as a metric gap.

All metrics use the namespace `AuraHistoria/Crawler`. Dimension values are ListingSource IDs, configured domains and bounded kinds, never URLs.

| Metric | Unit | Dimensions | Recorded |
|---|---|---|---|
| `ScrapesSucceeded`, `ScrapesFailed` | Count | `listing_source_id`, `domain` | Per scraper domain batch: URLs whose scrape completed (changed or not) and URLs whose scrape failed. |
| `DomainCircuitOpened` | Count | `listing_source_id`, `domain` | Per scraper domain batch: 1 when the batch opened the [domain circuit](scheduling-and-retries.md#scraper-domain-circuits), else 0. |
| `UrlFailures` | Count | `listing_source_id`, `domain`, `failure_kind` | Per scraper domain batch and failure kind: the URL failures that recorded that `last_error_kind`, such as `HTTP_503` or `Timeout`. |
| `ProductsCaptured` | Count | `listing_source_id`, `domain` | Per raw-capture batch: observations raw capture persisted durably; 0 when every capture of the domain failed. |
| `CrawlOutcome` | Count | `listing_source_id`, `domain`, `crawl_outcome` | 1 per spider crawl, with the `last_crawl_outcome` it [recorded](scheduling-and-retries.md#spider-crawl-completion). |
| `CrawlProductUrls`, `CrawlTruncated` | Count | `listing_source_id`, `domain` | Per spider crawl that classified its URLs with a product pattern, including a pattern that matched nothing: the product URLs found, and 1 when the crawl was cut short, else 0. |
| `ScraperDueUrls` | Count | none | URLs due for scraping when a scraper pass starts. |
| `ScraperPassDuration`, `SpiderPassDuration` | Milliseconds | none | Per scheduler pass, including a pass that finds no work. |

Create these alarms from the metrics; each is an operator requirement, not a deployed resource:

| Alarm | Metric and statistic | Period and threshold | Missing data |
|---|---|---|---|
| A ListingSource domain made no successful scrape in 24 hours | `ScrapesSucceeded` Sum per `listing_source_id`, `domain` | 1 day, `<= 0` | Breaching: a domain that is never scraped records nothing. |
| A product pattern matched no URL | `CrawlProductUrls` Maximum per `listing_source_id`, `domain` | 1 hour, `<= 0` | Ignore: the alarm holds until the next crawl records a count. |
| A scraper pass took more than 15 minutes, or none finished | `ScraperPassDuration` Maximum | 15 minutes, `> 900000` | Breaching: every pass records a duration, so silence means a stall. |
| The LLM governor made a call wait more than 60 seconds | Not recorded yet | 1 minute, `> 60000` | Not breaching |

The crawler does not record the LLM governor wait yet; with `CRAWLER_AUTHORING_MODE` `manual` or `agent` it makes no LLM call. The per-domain alarms need one alarm per configured domain, or one CloudWatch Metrics Insights alarm grouped by `listing_source_id` and `domain`.

## Validation

Run targeted checks after crawler changes:

```sh
cargo check -p crawler
cargo test -p crawler --all-features
cargo test -p crawler --tests
```

For database or local-flow changes, verify migrations, both script folders, the compose file, and affected integration tests together.
