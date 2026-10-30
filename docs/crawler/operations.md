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

The local `dev` binary runs the same pipeline through the shared [composition root](README.md#binaries) and reads the same LLM, spider and review settings. It replaces `BUSINESS_DATABASE_URL`, AWS credentials and CloudWatch export with demo ListingSources, a raw-observation file and stdout logging, and defaults `SPIDER_MAX_SIZE_BYTES` to 8 MiB when it is unset; `server` requires the variable and its spider rejects each crawl without it. Without `LOCAL_DB_URL`, `dev` starts the [local database](#local-database) itself and uses its `crawler_demo` database. With it, `dev` uses that database as given, like `server`: a run against a copy of `crawler_demo` leaves the demo state untouched, and a `.env` written for `server` points `dev` at the server database.

`server` and `dev` apply crawler-local migrations at startup. Migrations under [`src/crawler/migrations`](../../src/crawler/migrations/) are the authoritative crawler database contract; do not repair the live schema by hand. This lifecycle is outside the AWS Deploy/Migrate/Initialize workflows: manual Migrate owns only business PostgreSQL migrations, and FX-only Initialize never migrates crawler state.

The crawler scheduler is deployed as a single process. Its spider and scraper workers may process many domains concurrently, while process-local per-component domain locks plus URL and ListingSource locks coordinate work inside that process. Spider and scraper may work on the same domain concurrently, each bounded by its own per-domain throttle. Different domains remain concurrent; multi-process or horizontally scaled scheduler coordination is outside the current design.

## Stopping the crawler

`server` and `dev` stop gracefully on SIGTERM (`systemctl stop`) or SIGINT (Ctrl+C). The crawler logs `crawler.shutdown.started` naming the signal, stops the review console and starts no new spider or scraper pass, spider crawl or scraper domain batch; a running scraper batch finishes its current URL and defers the rest to a later run. Running work then has a grace period, 30 seconds by default, to finish, and raw observations already scraped are flushed to raw capture before the process exits 0 with `crawler.shutdown.completed`. Work still running when the grace period ends is abandoned and the process exits 0 with `crawler.shutdown.grace_period_expired`; an abandoned spider crawl or scraper URL records no outcome and is selected again after restart. A second signal during the grace period exits at once with 128 plus the signal number. Give the service manager a stop timeout longer than the grace period (systemd's default `TimeoutStopSec` of 90 seconds is enough) so it does not escalate to SIGKILL first. The review agent runner stops its agent processes and records their cost before it exits ([stop and undo](#stop-and-undo)).

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

The console fetches a shop's pages for a review in three live jobs ([review rail](extraction-and-review.md#review-rail)): a product-schema review's live matrix refresh (`GET /api/reviews/{id}/matrix?refresh=true`), a candidate evaluation (`POST /api/reviews/{id}/evaluate`) and a page projection (`GET /api/reviews/{id}/page-projection`). `CRAWLER_REVIEW_MAX_CONCURRENT_LIVE_JOBS` (default 4, a whole number of at least 1) caps how many run at once, for all reviews, humans and review agents together, so neither agents nor open console tabs multiply the load on the shops: a refresh or an evaluation holds its place until it finishes in the background, a projection until its page is projected, and a request that joins a running refresh takes none. Each job parses the pages it fetched on a blocking thread within its place, so the cap also bounds that parsing, and a slow, deeply nested page never holds up the console's other requests. A job above the cap is refused before it fetches anything with `429`, `reason` `LIVE_JOBS_BUSY`, the cap as `limit`, `retry_after_secs` and a `Retry-After` header; `Crawler review console listening` logs the cap as `max_concurrent_live_jobs`. Live previews, of any URL (`/api/live-inspect`, `/api/live-html`) and of a review page (`/api/review-pages/{id}/inspect`, `/html`), and review-page additions fetch one page each outside the cap. The live previews serve humans only and answer the [review agents](#review-agents)' tokens `401`, so no agent fetches a page through them past the cap or for a review that waits for a human; no `review-mcp` tool uses them, and the agents read a page as its projection (`inspect_page`) instead. A matrix read without `refresh` never fetches: it answers the stored matrix, `202` while a refresh runs, the last refresh's failure, or `404` with `reason` `NO_MATRIX` while the review stores no matrix of its current candidate. The console then offers **Evaluate on live pages**, and the agents call `refresh_matrix`; both ask for `refresh=true` explicitly.

`GET /api/listing_sources` is the operator's overview: each ListingSource with its pending reviews, URL counts and, per domain, the last crawl outcome, failure kind, failure count and next crawl time as [crawl completion](scheduling-and-retries.md#spider-crawl-completion) records them. It lists ListingSources with a domain waiting for an operator first, so a domain waiting for a review or for an operator is visible there without the logs. `GET /api/reviews`, the console's queue, lists at most 200 reviews: pending reviews of crawl-enabled ListingSources first, then pending reviews of disabled ones, which the console marks and which hold back no crawl or scrape, then closed reviews, newest first within each.

`GET /api/pending-reviews` is the agents' queue, which the [review agent runner](#review-agent-runner), its [digest](#digest) and `review-mcp`'s `list_pending_reviews` read instead: the pending reviews of crawl-enabled ListingSources only, oldest first by creation time and then review id, without their candidate and validation summary, page by page, so the agents work reviews in the order they opened and newer ones never keep the longest-waiting review from them. `human_required=false` or `true` lists only the reviews the agents can work or only those that wait for a human ([repair cap](#review-agents)), so those never take a workable review's place; `limit` is the page size (default 100, at most 500); `after=<review id>` reads the page after the review the previous page ended with, which that page names as `next_after` (`null` on the last page). A page starts after that review's creation time and id, so a review that closes or comes to wait for a human between two reads moves no other review across a page boundary, and an `after` that names no review answers `404`; any other value that is not accepted answers `400`. Every page answers `totals`, counted in SQL over the whole queue whatever page or filter was asked for: the pending reviews of crawl-enabled ListingSources the agents can work (`workable`) and those that wait for a human (`waiting_for_human`).

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
| `manual_url_pattern_required` | A domain needs a product URL pattern | Decision `UNDECIDED` with the crawled URL sample | Enter a regex or `NO_PATTERN` as the candidate, then approve. An undecided candidate cannot be approved. The crawl that opened the review finishes discovery and stores its URLs; approval classifies them with the decision and releases the domain, and when that crawl still runs, it classifies the URLs it stores with the decision too instead of waiting ([crawl completion](scheduling-and-retries.md#spider-crawl-completion)); a running crawl that has not classified yet uses the pattern too, without opening another review. |
| `manual_schema_required` | A ListingSource has no product schema, or its cached schemas stop matching a page | The cached schemas plus a generic starter schema, with the seed or triggering page | Rewrite the starter schema, check it with the live matrix, then approve. Approval replaces the ListingSource's schemas with the reviewed list. For a sold or withdrawn item the shop serves at HTTP 200 with a removal notice and no product markup, [declare the page removed](extraction-and-review.md#removed-page-templates) on the same review instead of authoring a schema for it; rejecting the review would classify it as a non-product page and leave its listing visible. |

A domain the spider records as `NEEDS_OPERATOR` ([repeated deterministic failures](scheduling-and-retries.md#scraper-domain-circuits)) opens no review: the console lists it above the review queue with its last failure kind and failure count, and the operator fixes the configuration (the right crawl root, or removing and re-registering the domain) or disables crawling for the ListingSource. A `NO_PATTERN` decision that turns out wrong is reset the same way.

A page that is not a product page should be excluded by narrowing the URL pattern. Rejecting a `manual_schema_required` review classifies its seed and triggering pages as non-product pages, with or without their [trailing slash](networking.md#stored-urls): a later scrape in `manual` or `agent` mode classifies such a page `other` before any cached schema extracts it, so it is not captured as a product even once another review approves a schema that extracts it, and the review is not reopened for it, until a human revokes the rejection; only a removal the scrape verifies first, an HTTP 404 or 410, a redirect to a non-product page or a matching [removed-page template](extraction-and-review.md#removed-page-templates), still captures it as removed. A listing captured from the page before stays visible with its last capture. Its waiting and reviewer-added pages keep their class. To keep a candidate that is not ready yet, give the review the needs-repair [verdict](extraction-and-review.md#review-rail) instead of rejecting it. A product page that leaves its title or state empty where a cached schema finds the element opens no review; it records a [missing required value](extraction-and-review.md#missing-required-values) instead, so an untitled product is never rejected as a non-product page.

A human revokes a mistaken rejection of a `manual_schema_required` review, the reviewer agent's included, with `POST /api/reviews/{id}/revoke-rejection` (optional `notes`) or the console's **Revoke rejection** action on the rejected review. The review stays rejected and keeps its history: the revocation is recorded as a `REJECTION_REVOKED` event of its actor, the console marks the review **Rejection revoked**, and from then on the rejection classifies none of its pages. It classifies the review's seed and triggering pages again with their domain's URL-pattern decision, as the spider classifies a URL it finds, and clears their retry delay, so a page the pattern takes is scraped again when due like any product page: captured when a cached schema extracts it, otherwise waiting on a new or pending `manual_schema_required` review. A page that another, unrevoked rejection covers is classified `other` again at that scrape. A rejection is revoked once: a second revocation answers `409`, one of any other review `400`, and an agent's `403` with `reason` `REVOKE_NEEDS_HUMAN`; no `review-mcp` tool revokes.

### Review agents

With `CRAWLER_AUTHORING_MODE=agent` the crawler works as with `manual`, and two review agents work its reviews through the review console as an operator does, each with a credential of its own: the author agent stages URL patterns and product schemas, the reviewer agent approves or rejects them. The [event trail](extraction-and-review.md#review-rail) records them as the actors `AGENT_AUTHOR` and `AGENT_REVIEWER`.

In `agent` mode the crawler does not guess what a product page is; Claude decides. The author agent labels URLs of a URL-pattern review's domain, its sample's among them, `PRODUCT` or `NOT_PRODUCT` with a reason and stages a pattern, and the reviewer agent's approval is taken only when the pattern follows those [sample labels](extraction-and-review.md#approval-guard). The spider, a URL-pattern review's URL sample and the classification that approving or restoring a pattern applies classify a URL by the domain's approved pattern alone: a URL the pattern takes is `product`, every other URL stays `other`, unclassified. The keyword classes `category`, `imprint` and `info` that the other modes derive from words in a URL, its host included, are never assigned. Until the pattern is approved every crawled URL stays unclassified, so nothing is scraped yet; the crawl stores its URLs and waits on the URL-pattern review it opens with its URL sample, as in [manual authoring](#manual-authoring), and approval classifies the stored URLs with the pattern.

| Request | Author agent | Reviewer agent |
|---|---|---|
| Console API reads, including a product-schema review's live matrix (`GET /api/reviews/{id}/matrix?refresh=true`), a page's projection (`GET /api/reviews/{id}/page-projection`) and the evaluation of a proposed candidate (`POST /api/reviews/{id}/evaluate`), which store nothing ([review rail](extraction-and-review.md#review-rail)) | Yes; the three live jobs not on a review that waits for a human (below) | As the author agent |
| Candidate and schema-field edits, removed-page declarations, sample labels (`POST /api/reviews/{id}/candidate`, `/schema-field`, `/sample-labels`), review page additions and removals (`POST /api/reviews/{id}/pages`, `DELETE /api/review-pages/{id}`) | Yes | `403` `REVIEWER_CANNOT_CHANGE_CANDIDATES` |
| Needs repair with its notes (`POST /api/reviews/{id}/needs-repair`) | Yes | Yes |
| Approve and reject (`POST /api/reviews/{id}/approve`, `/reject`) | `403` `AUTHOR_CANNOT_DECIDE` | Within the approval guard; a rejection needs non-blank `notes` (`400`) |
| Restore (`POST /api/reviews/{id}/restore`) and revoking a rejection (`/revoke-rejection`) | `403` `RESTORE_NEEDS_HUMAN`, `REVOKE_NEEDS_HUMAN` | `403` `RESTORE_NEEDS_HUMAN`, `REVOKE_NEEDS_HUMAN` |
| Browser sessions (`POST /api/session`, `/api/session/logout`), live previews (`GET /api/live-inspect`, `/api/live-html`, `/api/review-pages/{id}/inspect`, `/html`), domain registration and removal | `401` | `401` |

Nobody configures the agents' tokens. At every start in `agent` mode, `server` and `dev` generate a fresh 256-bit token per role from the operating system's CSPRNG and write both to `review-agent-tokens.json` in the crawler's working directory, its state directory (`/var/lib/crawler` under the [systemd unit](#systemd-unit)): `{"author": "<64 hex characters>", "reviewer": "<64 hex characters>"}`. The file is written under a temporary name with mode `0600` and renamed into place, so a reader never sees a partial file and only the crawler's user can read it; the [review MCP server](#review-mcp-server) runs as that user and reads its role's token from it. A start that cannot write the file fails. The console compares a bearer with every token in constant time. Tokens are never logged or printed: the startup log names the file's path, and `Crawler review console listening` logs `agent_access`. A restart replaces both tokens and so revokes the old ones. In every other mode no agent acts: the crawler writes no token file and removes one an earlier start left.

The agents always work next to a human. `server` in `agent` mode fails startup without `CRAWLER_REVIEW_AUTH_TOKEN`, so a human can reject, restore, revoke a rejection and override what the agents do. In `dev` the tokenless loopback console stands in for it: same-origin requests without a token keep acting as `HUMAN` while the agents send their tokens. A request without an agent's token never acts as an agent.

Agents change pending reviews only: their candidate and field edits, removed-page declarations, sample labels and page additions and removals on any other review, including the approved review whose candidate is live, answer `403` `AGENT_CHANGES_ONLY_PENDING_REVIEWS` and record nothing, so they change the live configuration only through an approval. The reviewer agent's approval is taken only when the [approval guard](extraction-and-review.md#approval-guard) passes, and one that carries `acknowledged_coverage` answers `403` `AGENT_COVERAGE_ACKNOWLEDGEMENT`. No agent approves its own work: an agent's approval is refused with `409` and `failed_checks` `[{"check": "APPROVER_IS_AUTHOR"}]` when the event trail shows that the approving agent changed the candidate version it approves or the review's pages; humans approve as before. `CRAWLER_REVIEW_AGENT_MAX_DECISIONS_PER_HOUR` (default 20) caps the agents' approvals plus rejections within any rolling hour, counted from the event trail so a restart does not reset it; above it the API answers `429` with `reason` `AGENT_DECISION_LIMIT`, the cap as `limit`, `retry_after_secs` and a `Retry-After` header. `CRAWLER_REVIEW_AGENT_MATRIX_MAX_AGE_MINUTES` (default 30) bounds how long ago the live matrix an agent approval of product schemas rests on fetched its pages; sample labels do not expire. `CRAWLER_REVIEW_AGENT_MIN_LABELLED_URLS` (default 3) is how many URLs an agent approval of a URL pattern needs labelled `PRODUCT`, and how many `NOT_PRODUCT`. `CRAWLER_REVIEW_AGENT_MAX_REPAIR_ROUNDS` (default 3) caps the agents' needs-repair rounds on one review, below. Empty or whitespace-only values count as unset; any other value that is not a whole number of at least 1 fails startup with the variable and the value, in every mode. At start the crawler builds one agent decision policy from `CRAWLER_AUTHORING_MODE` and these settings, and the console, the scraper and the spider all apply it. `GET /api/review-agent-limits` answers its four limits as `max_decisions_per_hour`, `matrix_max_age_minutes`, `min_labelled_urls` and `max_repair_rounds` to anyone who may read the console API, the agents' tokens included, so the [runner](#review-agent-runner) tells the agents the limits the console applies instead of keeping a copy of these settings. `Crawler review console listening` logs `agent_access` and the four limits as `agent_max_decisions_per_hour`, `agent_matrix_max_age_minutes`, `agent_min_labelled_urls` and `agent_max_repair_rounds`.

The agents' back-and-forth on a review is bounded. Each needs-repair verdict of either agent on a review is one round, counted from the event trail since the review's latest `HUMAN` event. The verdict that reaches `CRAWLER_REVIEW_AGENT_MAX_REPAIR_ROUNDS` is taken, and from then on the pending review waits for a human: every further change and verdict of an agent on it (candidate and field edits, removed-page declarations, sample labels, page additions and removals, needs-repair, approve and reject) and every [live job](#review-service) of an agent that fetches the shop's pages for it (a live matrix refresh, a candidate evaluation and a page projection) answers `409` with `reason` `HUMAN_REQUIRED`, `agent_rounds` and the cap as `limit`, and records and fetches nothing, so the agents spend no shop requests on a review they cannot change or decide. The console checks it under the review's row lock in the transaction of the refused request, the lock every review change holds while it records its event, so no agent request racing the verdict that reaches the cap gets past it; a matrix refresh is checked before it fetches and again when it stores. A refresh the store refuses this way answers only the agent whose refresh it was: a human, or the other agent, who joined it learns that it stored nothing, and the next matrix read of anyone else finds the matrix stored before, or `NO_MATRIX`, never the agents' refusal. Other reads stay open, and humans are unaffected. The review list, the [agents' queue](#review-service) and `GET /api/reviews/{id}` report `agent_rounds` and `human_required`, the console marks such a review **Needs a human**, `review-mcp`'s `get_review` passes both on to the agents while `list_pending_reviews` only counts such reviews, and the [runner](#review-agent-runner) starts no process for it and lists it in its [digest](#digest). A human hands the review back to the agents with any change or verdict of their own on it: an edit, a page or sample-label change, or, to tell the agents what to do, a needs-repair verdict with notes; the count restarts from that event. Approving or rejecting it closes it. `human_required` is reported only in `agent` mode: in every other mode no agent acts and a human decides every review, so it stays `false`, also for a review that an earlier `agent` start's agents left at the cap, and the agents' queue counts every pending review as `workable`; `agent_rounds` still counts their verdicts.

The kill switch needs no deploy: set `CRAWLER_AUTHORING_MODE=manual` and restart. The restart removes the token file, the agents' tokens answer `401`, and humans work the reviews as in [manual authoring](#manual-authoring); a human [restores](#review-service) any agent approval that should not stay live.

### Review MCP server

The agents reach the console only through `review-mcp`, an MCP server over stdio that the agents' MCP client starts once per agent process, in exactly one role: `review-mcp --role author` or `review-mcp --role reviewer`. `--review <review id>` scopes the process to that one review: it then lists no `list_pending_reviews` and refuses every tool call whose `review_id` names another review, or none, with `REVIEW_OUTSIDE_SCOPE`, before any request. The [runner](#review-agent-runner) always starts it scoped to the review of the agent process, so page content an agent reads cannot turn it on another review; without `--review`, for an MCP client a person drives, it works every review. Build it with `cargo build --release --locked -p crawler --bin review-mcp` next to `server` and run it as the crawler's user, the only one who can read the token file. It is a thin client of the console API: it has no database access and no review rule of its own, so the console's role refusals, [approval guard](extraction-and-review.md#approval-guard), decision cap and URL rules apply to every tool call.

| Setting | Meaning |
|---|---|
| `CRAWLER_REVIEW_CONSOLE_URL` | The console's base URL, default `http://127.0.0.1:7878`. Plain http is accepted for a loopback host only; credentials, a query or a fragment fail startup. |
| `CRAWLER_REVIEW_AGENT_TOKEN_FILE` | The token file, default `review-agent-tokens.json` in the working directory. Point it at the crawler's state directory, `/var/lib/crawler/review-agent-tokens.json` under the [systemd unit](#systemd-unit). |
| `LOG_LEVEL` | Level of its JSON log on stderr, default `info`. |

An empty or whitespace-only setting counts as unset. A missing or unknown `--role`, a `--review` that is no review id, a flag given twice, or any other argument, fails startup with exit code 2. The role's token comes from the token file only, never from a setting, an argument or a tool call: `review-mcp` reads it at startup, and fails when it cannot, as outside `agent` mode, and reads it again when the console answers `401`, retrying the request once when a crawler restart has rotated the token. It connects to the console directly, ignoring proxy settings, and follows no redirect, so the token reaches only the configured console. stdout carries the MCP session only; the log names each tool call with its outcome and duration, never its arguments, its answer, a page or a token.

A role's server lists and runs only its own tools; the tool descriptions in [`review_mcp/tools`](../../src/crawler/src/review_mcp/tools) are the agents' contract. `list_pending_reviews` answers one page of the [agents' queue](#review-service) with `human_required=false`: at most 50 of the pending reviews of crawl-enabled ListingSources the agents can work, oldest first, each with its `agent_rounds`, with `total_pending`, the count of all of them, `waiting_for_human`, the count of those that wait for a human, which it does not list, and `more`; when `more` is true, or `truncations` names the list, the agent reads on with `after` set to the `review_id` of the last review listed. `get_review` reports a review's `agent_rounds` and `human_required`, and both tools tell the agents to leave a review that waits for a human. `review-mcp` keeps no limit of its own: the numbers it passes on are the console's, in its refusals' `limit` and a failed `TOO_FEW_LABELS` check's `minimum`.

| Role | Tools |
|---|---|
| Both | `list_pending_reviews` (not when scoped with `--review`), `get_review`, `inspect_page`, `refresh_matrix`, `mark_needs_repair` |
| Author | `evaluate_candidate`, `update_candidate`, `update_schema_field`, `add_review_page`, `remove_review_page`, `declare_removed_page`, `label_sample_urls` |
| Reviewer | `approve`, `reject` |

A console refusal comes back as a tool error whose text is a JSON object with `kind`, `error`, `status` and the console's own `reason`, `failed_checks`, `candidate_version`, `coverage`, `retry_after_secs`, `limit`, `agent_rounds` and `details`. A tool taking a URL refuses one that is not a plain http(s) URL on a domain of the review's ListingSource (`URL_OUTSIDE_REVIEW_SOURCE`) before any request, and the console refuses it again. No answer carries a token, cookie or header. Every answer stays below 48 KiB of JSON, so it fits the 50,000 characters Claude Code keeps of a tool result in the agent's context (a longer one is replaced by a preview of a file the agents cannot read): long texts end in a `[truncated: ...]` marker, `inspect_page` answers at most 36,000 characters of a page's projection (fewer for text in non-Latin scripts), with a marker line where it stops and an `offset` to read on from there, and each shortened list is named in the answer's `truncations`. A review's `candidate` is never shortened, because `update_candidate` takes it back whole; when the rest cannot be cut enough, the tool answers `ANSWER_TOO_LARGE`. `refresh_matrix` and `evaluate_candidate` wait for the console's background work for up to 15 minutes, then answer `STILL_RUNNING`.

For the Claude Code CLI, one role's MCP configuration reads:

```json
{"mcpServers": {"crawler_review": {"command": "/opt/crawler/bin/review-mcp", "args": ["--role", "author"],
  "env": {"CRAWLER_REVIEW_AGENT_TOKEN_FILE": "/var/lib/crawler/review-agent-tokens.json"}}}}
```

### Review agent runner

`review-agent-runner` works every pending review with the two review agents, end to end and with no human in the loop: per review a fresh author agent process writes, tests, labels and stages the candidate, then a fresh reviewer agent process approves, rejects or gives needs-repair within the [approval guard](extraction-and-review.md#approval-guard). Each process is a headless `claude -p` of the Claude Code CLI that is logged in on the machine, with only its role's [review MCP server](#review-mcp-server) as tools. The runner is an operator of the console like a human with a browser: it lists reviews and events with the author agent's token, has no database access and makes no review decision itself; the crawler makes no LLM call. Humans oversee after the fact through the [event trail](extraction-and-review.md#review-rail) and the [digest](#digest). Its code is the `crawler` crate's [`review_agent_runner`](../../src/crawler/src/review_agent_runner/mod.rs), outside the crawler's services.

#### Team seat and CLI login

The agents run on a dedicated member of Aura's Claude for Teams plan, never on a person's seat, so they draw only from that seat's five-hour and weekly allowance; nobody else signs in to it. A Team admin invites the crawler member (for example a `crawler@` mailbox), gives it a Premium seat and leaves usage credits off; an admin can turn them on with an individual spend limit in **Organization settings > Usage** when a busy week needs more.

On the crawler host, install a pinned Claude Code version (auto-update off) outside `/home` and `/root`, which the runner's [sandbox](#sandbox) hides, for example as `/usr/local/bin/claude`, and sign in once as the crawler member with the `HOME` and crawler-only configuration directory the service uses:

```sh
install -d -m 0700 -o crawler -g crawler /var/lib/crawler-review-agents
sudo -u crawler env HOME=/var/lib/crawler-review-agents CLAUDE_CONFIG_DIR=/var/lib/crawler-review-agents/claude claude   # then /login with the crawler member
sudo -u crawler env HOME=/var/lib/crawler-review-agents CLAUDE_CONFIG_DIR=/var/lib/crawler-review-agents/claude claude auth status --json
```

The status must show `"loggedIn": true`, `"authMethod": "claude.ai"` and Aura's organization. No `claude setup-token` and no `CLAUDE_CODE_OAUTH_TOKEN` are needed; a `CLAUDE_CODE_OAUTH_TOKEN` in the runner's environment is used if present (`authMethod` `oauth_token`). Every run starts with this probe and refuses to run, with the reason in the journal, unless the CLI is logged in with a Claude subscription; when the login expires, sign in again the same way. `ANTHROPIC_API_KEY` and `ANTHROPIC_AUTH_TOKEN` must not be set for the runner: in print mode they silently take over billing. The runner never passes them, or any other variable outside its [allowlist](#invocation), to a child.

#### Production and development

In production the crawler runs `server` with `CRAWLER_AUTHORING_MODE=agent` from the start, and the runner runs on the same host. Build `review-agent-runner` and `review-mcp` with `server` (`cargo build --release --locked -p crawler --bin server --bin review-mcp --bin review-agent-runner`), copy both to `/opt/crawler/bin/`, and install the example unit [`review-agent-runner.service`](../../src/crawler/scripts/linux/review-agent-runner.service) and the environment file [`review-agent-runner.env.example`](../../src/crawler/review-agent-runner.env.example) as `/etc/crawler/review-agent-runner.env`, then `systemctl daemon-reload && systemctl enable --now review-agent-runner`. The service runs as the `crawler` user, the only one who can read the token file, with its own state directory `/var/lib/crawler-review-agents` and the CLI login there; its environment file holds runner settings only, never the crawler's, so no agent process inherits the console's human token. The runner starts a run, rests `CRAWLER_REVIEW_AGENT_RUNNER_INTERVAL_SECONDS` after it ends, and starts the next until it is stopped. A run that fails, such as on an unreachable console or an expired login, is logged and the next follows after the interval; systemd restarts a runner that crashed, but not one that refused its settings (exit code 2). A run with no review to work makes no agent call: it lists the queue and probes the login.

For development, build the runner and `review-mcp` with `dev` and run `dev` with `CRAWLER_AUTHORING_MODE=agent` and the developer's own logged-in CLI:

```sh
cargo build --locked -p crawler --bin dev --bin review-mcp --bin review-agent-runner
CRAWLER_AUTHORING_MODE=agent target/debug/dev
```

`dev` then starts `review-agent-runner` from next to itself as its child, as systemd runs it next to `server`: in `dev`'s working directory, where `dev` writes the token file, from an empty environment holding only the runner's settings (`CRAWLER_REVIEW_AGENT_*`, `CRAWLER_REVIEW_CONSOLE_URL`, `LOG_LEVEL`) and the variables of the [allowlist](#invocation), never the crawler's database URLs, cloud credentials or `CRAWLER_REVIEW_AUTH_TOKEN`. Unless they are set, the runner works the console `dev` serves every 30 seconds and keeps its state in `target/review-agents`. Its log lines join `dev`'s, and stopping `dev` stops it as `systemctl stop` stops the service ([stop and undo](#stop-and-undo)): a Ctrl+C in `dev`'s terminal reaches the runner at once, and `dev` sends it SIGTERM once its own [shutdown](#stopping-the-crawler) ended, waiting up to 30 seconds for it to exit. The agents spend the developer's own seat. With `CRAWLER_DEV_START_REVIEW_AGENT_RUNNER=false`, `dev` starts no runner; `review-agent-runner --once`, started by hand from the same directory, works the pending reviews once and then writes a digest of the last day at once. `manual` mode stays for development without agents.

#### Sandbox

The agent processes read untrusted shop pages, so the example unit runs the runner and everything it starts in a systemd sandbox. `ProtectSystem=strict` makes the whole file system read-only except `ReadWritePaths=/var/lib/crawler-review-agents`, the runner's state directory, and the private `/tmp` of `PrivateTmp=true`; `ProtectHome=yes` hides `/home`, `/root` and `/run/user`. The unit points `HOME` and `CLAUDE_CONFIG_DIR` into the state directory, so the CLI keeps its login, settings and caches there; the runner itself writes only its state there and its agents' empty working directories in `/tmp`, and reads the token file in `/var/lib/crawler`. A `review-agent-runner --once` started from a shell runs outside the sandbox.

A CLI version can start writing somewhere new, so verify the sandbox with the real login after installing or changing the unit and after every CLI upgrade:

1. `systemctl daemon-reload && systemctl restart review-agent-runner`; the restart starts a run at once.
2. `journalctl -u review-agent-runner -n 50 -o cat` shows `Review agent runner started with the CLI's Claude login`: the login probe read the login inside the sandbox. A line with `the runner refuses to start` names what failed.
3. Once a workable review is pending (a run with none makes no agent call), `journalctl -u review-agent-runner -f -o cat` shows `Review agent process finished` with `end` `REPORTED`, `LIMIT_REACHED` or `USAGE_LIMIT`, and `tail -n 1 /var/lib/crawler-review-agents/review-agent-processes.jsonl` shows its record. `FAILED` after `The review agent process printed no result envelope` points at the sandbox when its `stderr_tail` names a path.
4. `journalctl -u review-agent-runner --since -1h -o cat | grep -i -E 'read-only file system|EROFS|permission denied|EACCES'` prints nothing.

`systemd-analyze verify` checks only the unit file; only these steps show that the CLI logs in and runs inside the sandbox. When a step fails because the CLI needs a path the sandbox denies, add that path to `ReadWritePaths` in a drop-in (`systemctl edit review-agent-runner`) rather than relaxing `ProtectSystem` or `ProtectHome`, and repeat the steps.

#### Invocation

Per review and role the runner starts, from a new empty directory of its own outside any project, with the prompt on stdin:

```text
claude -p --output-format stream-json --verbose --json-schema <role's report schema>
  --mcp-config <role's review-mcp> --strict-mcp-config --allowedTools "mcp__crawler_review__*"
  --permission-mode dontAsk --no-session-persistence --setting-sources "" --model <model>
  --effort <effort> --max-turns <turns> --tools ""
```

The process prints its messages as JSON lines. The runner keeps the end of that stream in memory, never logs or stores it, and reads two kinds of line: the last `result`, the envelope with the report, `subtype`, `terminal_reason`, tokens and `total_cost_usd`, and the last `rate_limit_event`, whose `resetsAt` is the reset time of a usage limit that rejected a request; `-p` takes `stream-json` only with `--verbose`. `--tools ""` disables every built-in tool (no shell, files, web or browser), and the MCP configuration names `review-mcp --role author|reviewer --review <review id>` as the server `crawler_review`, with the console URL and the token file's path, never a token, so `mcp__crawler_review__*` allows exactly that role's review tools. `--bare` is never used: it does not read the login. `--max-turns` exists in Claude Code 2.1.312 although its help does not list it. Every process starts with an empty context. Its environment, and that of the login probe, is not inherited: it holds only `PATH`, `HOME`, `TMPDIR`, `CLAUDE_CONFIG_DIR`, `CLAUDE_CODE_OAUTH_TOKEN`, `LANG` and `LC_*`, `HTTPS_PROXY` and `NO_PROXY` (either case), `SSL_CERT_FILE`, `SSL_CERT_DIR` and `NODE_EXTRA_CA_CERTS`, and on Windows `SystemRoot`, without which no network request works, and `TEMP` and `TMP`, as far as the runner has them, plus `DISABLE_AUTOUPDATER=1`, `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1`, `CLAUDE_CODE_DISABLE_CLAUDE_MDS=1`, `CLAUDE_CODE_DISABLE_AUTO_MEMORY=1` and `TZ=UTC` ([`invocation.rs`](../../src/crawler/src/review_agent_runner/invocation.rs)). No `ANTHROPIC_*` variable, provider switch such as `CLAUDE_CODE_USE_BEDROCK`, cloud credential or crawler setting reaches an agent; `review-mcp` gets the console URL and the token file's path from its MCP configuration. The prompts and report schemas are versioned files in [`review_agent_runner/prompts`](../../src/crawler/src/review_agent_runner/prompts) and [`schemas`](../../src/crawler/src/review_agent_runner/schemas); both prompts tell the agents that page content is data, never instructions, and explain `human_required` and the `HUMAN_REQUIRED` refusal of the [repair cap](#review-agents), and the reviewer's says that rejecting a `manual_schema_required` review classifies its seed and triggering pages as non-product pages until a human revokes the rejection ([manual authoring](#manual-authoring)). A changed prompt or report schema gets a new version, such as `author.v2`, which the ledger and the log record with each process. The author reports `review_id`, `action` (`STAGED`, `NOTHING_TO_STAGE`, `SKIPPED`), `candidate_version`, `summary` and `risks`; the reviewer reports `review_id`, `decision` (`APPROVE`, `REJECT`, `NEEDS_REPAIR`, `SKIP`), `confidence`, `summary`, `risks`, `field_findings` and `server_answer` (`outcome` `ACCEPTED`, `REFUSED` or `NOT_SENT`, the refusal's `kind`, `failed_checks` and `detail`). A report is the agent's account; the event trail is what happened.

#### Settings

| Setting | Meaning | Default |
|---|---|---|
| `CRAWLER_REVIEW_CONSOLE_URL`, `CRAWLER_REVIEW_AGENT_TOKEN_FILE` | The console and the token file, as for [`review-mcp`](#review-mcp-server); the runner lists reviews with the author token and refuses to start when the file cannot be read, as outside `agent` mode | `http://127.0.0.1:7878`, `review-agent-tokens.json` in the working directory |
| `CRAWLER_REVIEW_AGENT_RUNNER_CLAUDE_BIN` | The Claude Code CLI, an executable: on Windows the runner cannot start npm's `claude.cmd` shim, so name the package's `bin\claude.exe` | `claude` on the `PATH` |
| `CRAWLER_REVIEW_AGENT_RUNNER_REVIEW_MCP_BIN` | The `review-mcp` binary | the one next to the runner |
| `CRAWLER_REVIEW_AGENT_RUNNER_STATE_DIR` | Lock, state, process ledger and digests | the working directory |
| `CRAWLER_REVIEW_AGENT_AUTHOR_MODEL`, `CRAWLER_REVIEW_AGENT_REVIEWER_MODEL` | Model alias or name per role | `claude-opus-5-5` |
| `CRAWLER_REVIEW_AGENT_AUTHOR_EFFORT`, `CRAWLER_REVIEW_AGENT_REVIEWER_EFFORT` | `low`, `medium`, `high`, `xhigh` or `max` | `high` |
| `CRAWLER_REVIEW_AGENT_AUTHOR_MAX_TURNS`, `CRAWLER_REVIEW_AGENT_REVIEWER_MAX_TURNS` | Turn cap per process | `80`, `50` |
| `CRAWLER_REVIEW_AGENT_RUNNER_INTERVAL_SECONDS` | Rest between the end of one run and the start of the next; `--once` ignores it | `300` |
| `CRAWLER_REVIEW_AGENT_RUNNER_MAX_REVIEWS_PER_RUN` | Reviews one run works | `4` |
| `CRAWLER_REVIEW_AGENT_RUNNER_MAX_PROCESSES` | Agent processes at once | `2` |
| `CRAWLER_REVIEW_AGENT_RUNNER_COOLDOWN_MINUTES` | How long a review that ended without a decision, and does not wait for a human, waits | `180` |
| `CRAWLER_REVIEW_AGENT_RUNNER_PROCESS_TIMEOUT_MINUTES` | Wall-clock limit of one process; a process over it is stopped with its `review-mcp` | `60` |
| `LOG_LEVEL` | JSON log on stdout, in the journal | `info` |

An empty or whitespace-only setting counts as unset; any other value that is not accepted, or any argument but `--once`, fails startup with exit code 2.

The limits of the agents' decisions are the console's, never the runner's: after the login probe each run reads them from `GET /api/review-agent-limits` with the author token, and the author's prompt asks for the console's `min_labelled_urls` labels of each kind. A run that cannot read them fails and starts no agent process, as every run does against the console of an older build, which does not answer them: deploy the runner and `review-mcp` from the build of `server` ([upgrade](#upgrade)). The runner reads no limit from its own environment: a `CRAWLER_REVIEW_AGENT_*` limit in its environment file has no effect.

#### Limits

- A run takes the lock `review-agent-runner.lock` in the state directory and releases it when it ends, so a `--once` started during the service's run does nothing and exits 0, and one started between two runs works.
- A run works at most the configured reviews: pending product-schema and URL-pattern reviews of crawl-enabled ListingSources, leaving out reviews that are cooling down and reviews that wait for a human ([repair cap](#review-agents)). It reads every page of the [agents' queue](#review-service), the reviews the agents can work and then those that wait for a human, so it sees every pending review however many there are, and none that waits for a human takes a workable review's place; a review that comes to wait for a human between the two reads counts as waiting. A review that waits for a human is no agent work: the run starts no process for it and gives it no cooldown, so it is workable again as soon as a human hands it back, and `Review agents pass started` and `Review agents pass finished` count it as `waiting_for_human`. Of the others, reviews no agent process worked in the ledger's eight days come first, oldest first, then those worked longest ago, so a review handed back and forth with needs-repair verdicts never holds the others back. A review of a disabled ListingSource holds nothing back and waits until the source is crawled again.
- The reviewer runs when the author staged a candidate or found nothing to stage and the review does not wait for a human after the author's pass. The runner reads the review after each author process, whether it reported or not: a review that the author's own needs-repair verdict handed to a human at the repair cap gets no reviewer process, whose every verdict the console would refuse with `HUMAN_REQUIRED`, and `Review agents pass finished` counts it as `handed_to_human`. A process that hits its turn or context limit ends without a decision, as does one that fails, outlives its timeout or reports no usable report, and an author `SKIPPED`: the runner records the envelope's `subtype` and `terminal_reason`, the review stays pending, and it cools down before its next pass. After each reviewer process the runner reads the event trail since the process started: the reviewer decided when it shows an `AGENT_REVIEWER` approval, rejection or needs-repair verdict on the review, whatever the report says the console answered, and otherwise, or when the trail cannot be read, the review cools down. A review that waits for a human after either process gets no cooldown, whichever agent's verdict brought it to the cap, as the runner reads the review then; one the runner cannot read counts as not waiting, so it gets its reviewer or cools down. A needs-repair verdict the trail records is worked again on the next run.
- When a process ends on a usage limit of the login, a session (five-hour), weekly or model limit or a spend limit, the runner starts no further process and skips work until the limit resets. The reset time is the `resetsAt` of the `rate_limit_event` that rejected the request; without one the runner reads it from the CLI's message, such as `You've hit your weekly limit · resets Mon 12:00am`, in UTC, the zone it starts the CLI in. A reset time it cannot read pauses it for an hour and logs the message's line at WARN, so a changed wording shows in the journal. Pending reviews simply wait, and cached schemas keep working.
- The runner sets no dollar limit of its own: the agents run on the login's subscription, whose usage limits above are the only spending stop. Each process's `total_cost_usd` is still recorded and summed over the rolling five hours and seven days for the log and the digest.
- The CLI reports a process's cost only in its result envelope. A process that outlives its timeout, or runs when the runner [stops](#stop-and-undo), gets SIGINT, on which the CLI ends its turn and prints its envelope with the cost so far; SIGTERM would end it without one. Once it has closed its output, or five seconds later, its process group gets SIGTERM, and SIGKILL five seconds after that; what it prints until then is read. A process stopped at its timeout or by the runner's stop, or killed by a signal, that reported no cost is charged the highest cost a process reported in the ledger's eight days; the ledger and `Review agent process finished` mark it `cost_charged`, the digest `cost charged`, and the rolling sums count it like a reported cost. The charge is an estimate, not an upper bound: a process that ran longer than any that reported may have spent more, and while the ledger holds no reported cost the charge is zero. The login's usage limits stay the only spending stop.

State between runs is in the state directory: `review-agent-runner-state.json` (pause, cooldowns, last digest), written as soon as a pause or cooldown is set, and the process ledger `review-agent-processes.jsonl`, one record per process with its review, role, prompt version, outcome, report, tokens and cost, appended when the process ends and kept for eight days. A state or ledger the runner cannot read fails the run instead of forgetting a pause or a cooldown; only a last ledger line an interrupted run left unfinished is completed or cut off.

#### Logging

Each process logs `Starting a review agent process` with its review, role, prompt version, model, effort and turn cap, and `Review agent process finished` with its outcome, `subtype`, `terminal_reason`, `api_error_status`, turns, duration, models, `usage` tokens, `total_cost_usd`, `cost_charged` and report; each run logs `Review agents pass finished` with its counts and the rolling cost sums, and a run of the service that fails logs `Review agent run failed; the next run follows after the interval` with its error, or `Review agent run failed while the runner stops`. The log never holds a prompt, page content, a raw envelope or a token; a process that prints no envelope logs its exit code and the last line of its stderr, shortened, and a usage limit without a reset time the runner can read logs `The CLI reported a usage limit without a reset time the runner can read; the agents pause for an hour` with the line of the CLI's message as `limit_message`.

#### Digest

Once a day a run of the service writes a digest of the agents' work since the previous one to `digests/review-agents-<time>.md` in the state directory and logs it as `Review agents digest`: first, under **Needs a human**, every pending review of crawl-enabled ListingSources that waits for a human after the run's pass, read page by page from the [agents' queue](#review-service), each with its review, artifact type, shop, agent rounds and last needs-repair note and time (`needs_a_human` in the log line); then approvals, rejections and needs-repair verdicts by agents and by humans, restores and revoked rejections, from the console's event trail of all reviews (`GET /api/review-events`, read page by page), the server's refusals of the reviewer's decisions by failed check, each process's outcome, and each review's tokens and cost, from the reports, with the rolling cost sums and any pause. Operators read the file on the host and act on what it shows: restore an approval, revoke a rejection, or reject or repair a review by hand.

#### Stop and undo

- Stop the agents: `systemctl disable --now review-agent-runner`. The unit's `KillMode=mixed` sends SIGTERM to the runner alone, never straight to the agent processes, as on every `systemctl stop` or restart. The runner logs `Review agent runner stopping`, starts no further process and stops each running one as at its [timeout](#limits), SIGINT first, so the ledger records it with its reported or charged cost and the `failure` `stopped when the runner stopped`. Its review does not cool down, the stopped run writes no digest, and the cooldowns and pauses already set are kept. The runner exits with 143 within 20 seconds; whatever still runs then is killed, and a second signal does not shorten the stop. `TimeoutStopSec=30s` leaves room for that before systemd kills what is left of the unit. Ctrl+C stops `--once` the same way, with exit code 130. Or switch the crawler to `CRAWLER_AUTHORING_MODE=manual` and restart it: the token file is removed and every run refuses to start. Neither needs a deploy.
- Undo an agent's rejection of a `manual_schema_required` review: a human revokes it ([manual authoring](#manual-authoring)), so its seed and triggering pages are scraped again and may open a new review. Other rejections classify no page: the next crawl or scrape opens a new review.
- Undo an agent's approval: a human restores it ([review service](#review-service)). A needs-repair verdict leaves the review pending, and a human can approve, reject or edit it in the console at any time; a change or verdict of theirs hands a review that waits for a human back to the agents ([review agents](#review-agents)).

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

Launch with `CRAWLER_AUTHORING_MODE=agent` and the [review agent runner](#review-agent-runner) on the host, or `manual` to work the reviews by hand: neither makes an LLM call or reads a Vertex AI setting ([`config.rs`](../../src/crawler/src/runtime/config.rs)). A later switch to `review` or `auto` makes `VERTEX_AI_PROJECT_ID`, `VERTEX_AI_LOCATION` and Application Default Credentials on the host required; startup fails without them.

### Review console access

[`ReviewServerConfig`](../../src/crawler/src/review/server.rs) applies these rules:

- `CRAWLER_REVIEW_BIND_ADDR` defaults to `127.0.0.1:7878`; any value that is not `host:port`, including an empty one, fails startup.
- An empty or whitespace-only `CRAWLER_REVIEW_AUTH_TOKEN` counts as unset. A non-loopback bind without a token fails startup with `CRAWLER_REVIEW_AUTH_TOKEN is required for non-loopback bind addresses`.
- With `CRAWLER_AUTHORING_MODE=agent`, a missing `CRAWLER_REVIEW_AUTH_TOKEN` fails startup with `CRAWLER_AUTHORING_MODE=agent needs CRAWLER_REVIEW_AUTH_TOKEN, ...`, and the `crawler` user must be able to write the working directory, which `StateDirectory=crawler` provides, for the agents' token file.
- A decision cap, matrix age, minimum of labelled URLs, repair-round cap or live-job cap (`CRAWLER_REVIEW_MAX_CONCURRENT_LIVE_JOBS`) that is not a whole number of at least 1 fails startup.
- `server` accepts review mutations only with the bearer token, so approving manual reviews needs the token even on loopback. Without a token, reads are open to anyone who reaches the port.

For launch, keep the loopback bind, set a random token, and tunnel: `ssh -L 7878:127.0.0.1:7878 <host>`, then open `http://127.0.0.1:7878`. The alternative is a non-loopback bind behind a TLS-terminating reverse proxy; the token is then mandatory and the session cookie is `Secure`, so browsers must use HTTPS.

### Logging

Logs are JSON on stdout, stored by the journal (`journalctl -u crawler-server`). `LOG_LEVEL` sets the base filter level (default `info`) in [`server.rs`](../../src/crawler/src/bin/server.rs); `RUST_LOG` is not read.

Setting `CRAWLER_CLOUDWATCH_LOG_GROUP` adds CloudWatch Logs export, which needs `logs:CreateLogGroup`, `logs:CreateLogStream` and `logs:PutLogEvents`; `server` creates the group and stream at every start and fails startup if that fails or either name is blank ([`logging.rs`](../../src/crawler/src/logging.rs)). Set `CRAWLER_CLOUDWATCH_LOG_STREAM`: its fallback, `HOSTNAME`, is not set by systemd, so the stream would be `unknown-host`. Credentials and region come from the AWS SDK default chain: an EC2 instance profile, a profile in the service user's `~/.aws` selected with `AWS_PROFILE` (including `credential_process`, for example IAM Roles Anywhere off AWS), or access keys in the environment file. Set `AWS_REGION` unless the profile or instance provides it.

### systemd unit

The example unit runs `server` as the unprivileged `crawler` user with `EnvironmentFile=/etc/crawler/crawler.env`, `Restart=on-failure`, `KillSignal=SIGTERM` and `TimeoutStopSec=45s`, a margin above the 30-second graceful-shutdown period before systemd sends SIGKILL. Its working directory is the empty state directory because `server` also loads a `.env` from its working directory or a parent; variables already in the environment take precedence. It starts after `network-online.target` and after `postgresql.service` when Postgres runs on the same host. A database that is still unreachable at start fails startup, and `Restart=on-failure` retries.

Run exactly one instance: no second host and no manual `server` next to the unit. The scheduler coordinates only through process-local locks ([scheduling and retries](scheduling-and-retries.md)), so a second process would crawl and capture the same work concurrently.

### Upgrade

Run `systemctl stop crawler-server`, replace the binary and run `systemctl start crawler-server`. Replace `review-mcp` and `review-agent-runner` from the same build and restart `review-agent-runner` with it: the runner reads the agents' limits from the console ([review agent runner](#settings)) and fails every run against a console that does not answer them. Pending crawler-local migrations apply at startup; never repair the schema by hand. Startup refuses a database with an applied migration the binary does not contain or whose checksum changed, so rolling back to an older binary after a migration requires restoring the database. The same refusal applies when a release squashes crawler-local migrations into the baseline `20260101000000_initial_schema.sql`, which its release notes state: no database that applied the earlier chain can be upgraded. The journal shows `Failed to run database migrations` and `Restart=on-failure` keeps restarting the unit. For such a release, stop the unit, drop and recreate the crawler-local database as its owner (`db-reset` locally, see [Local database](#local-database)), then start the new binary. This discards all crawler-local state: URL state, approved URL patterns and schemas, and reviews with their event trail. A binary that raises the [extractor version](extraction-and-review.md#extraction-contract), or changes what the page hash or the scraper-context fingerprint covers, extracts every page once more at its next scheduled scrape and validates its images again, and one that changes raw extraction does so for every page whose extraction it changes, so expect longer scraper passes until each such page has been scraped once.

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
| `ScrapesSucceeded`, `ScrapesFailed`, `ScrapesAwaitingSchemaReview` | Count | `listing_source_id`, `domain` | Per scraper domain batch: URLs whose scrape completed (changed or not), URLs whose scrape failed, and URLs whose page [waits on a product-schema review](scheduling-and-retries.md#fetch-retries), which is no failure and counts in neither `ScrapesFailed` nor `UrlFailures`. |
| `DomainCircuitOpened` | Count | `listing_source_id`, `domain` | Per scraper domain batch: 1 when the batch opened the [domain circuit](scheduling-and-retries.md#scraper-domain-circuits), else 0. |
| `UrlFailures` | Count | `listing_source_id`, `domain`, `failure_kind` | Per scraper domain batch and failure kind: the URL failures that recorded that `last_error_kind`, such as `HTTP_503` or `Timeout`. |
| `ProductsCaptured` | Count | `listing_source_id`, `domain` | Per raw-capture batch: observations raw capture persisted durably; 0 when every capture of the domain failed. |
| `CrawlOutcome` | Count | `listing_source_id`, `domain`, `crawl_outcome` | 1 per spider crawl that [recorded](scheduling-and-retries.md#spider-crawl-completion) a `last_crawl_outcome`, with that outcome. |
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
