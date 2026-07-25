# ADR: Hetzner/Postgres/Sequin migration

Status: draft for review  
Parent: #1341  
Covers: #1368, #1369, #1374

## Decision

Move most backend business runtime from AWS serverless/DynamoDB to a Hetzner-friendly service layout:

- `aura-historia-api` — REST API process.
- `aura-historia-worker` — async worker process.
- Postgres — business source of truth.
- Sequin — Postgres CDC delivery into the worker.
- OpenSearch — rebuildable search/percolator projection.
- CloudFront — public edge in front of the Hetzner API origin.

Design must not assume one machine. The default deployment may run all services on one host, but API, worker, Postgres, Sequin, and OpenSearch must be configurable as separate hosts.

Design must keep a later move back to AWS practical. API and worker should be normal container-friendly Rust processes. Business logic should depend on repositories and domain jobs, not Hetzner-specific infrastructure.

## Non-goals

- No API Gateway adapter while migrating API routes.
- No product command SQS buffer for migrated writes.
- No users OpenSearch index in target design.
- No crawler migration. Crawler is separate and out of scope.
- No exactly-once external side effects. Use idempotency, retry, and repair/backfill.

## AWS stays

| AWS part | Target role |
|---|---|
| Cognito | Identity provider only. API verifies JWTs directly. |
| DynamoDB notifications | Notification TTL and insert-to-send workflow. |
| DynamoDB access tokens | Keep existing access-token storage and lookup. |
| DynamoDB OAuth clients and codes | Keep OAuth clients, authorization codes, and third-party exchange codes. |
| `notification-send` Lambda | Sends external notifications through SES/S3 template flow. |
| Step Functions | Partner-shop-application workflow engine. Its Lambda writes business rows to Postgres directly. |
| Shopify Lambda | Shopify event intake stays in AWS, then writes product rows/events to Postgres directly. Its AWS SQS queue is external intake buffering, not a product command buffer. |
| Stripe Lambda | Stripe subscription event intake stays in AWS, writes user rows to Postgres directly. |
| FxRate Lambda | Scheduled FX-rate update stays in AWS/DynamoDB. |
| CloudWatch log-retention Lambda | Stays for AWS logs. |
| CloudFront/WAF | Public edge for Hetzner API origin. |

## AWS Lambda to Postgres access

AWS survivor Lambdas that need business data connect to Postgres directly.

Minimum rules:

- Use TLS to Postgres.
- Keep credentials in AWS SSM/Secrets Manager or equivalent secret source.
- Keep connection pools very small in Lambda runtimes.
- Put a connection pooler such as PgBouncer in front of Postgres if direct Lambda concurrency can exceed safe Postgres connections.
- Restrict network path by allowlist, VPN, tunnel, private overlay, or another reviewed control.
- Monitor failed connects, connection count, pool wait, and auth failures.

## Business source of truth

Postgres owns:

- users
- shops
- partner-shop-applications
- products
- product-events
- product-watchlist
- search-filters
- search-filter matches
- worker processed-job markers, dead letters, and schedules

DynamoDB owns:

- notifications
- access tokens
- OAuth clients
- OAuth authorization codes
- OAuth third-party exchange codes
- FX rate

OpenSearch owns only rebuildable projections:

- products
- shops
- search-filter percolator documents

## Product writes

Product writes are synchronous.

API/webhook/Shopify write flow:

1. Authorize request.
2. Load needed Postgres rows and FX rate.
3. Decide domain event(s).
4. In one Postgres transaction:
   - insert `product_events`
   - update `products` materialized row, including `products.event_id`
5. Commit.
6. Return success/failure to caller.
7. Sequin delivers committed changes to `aura-historia-worker`.

No intermediate product command queue. No `202 accepted because queued` semantics for migrated product writes.

## CDC and worker delivery

There is no durable `worker_inbox` table.

A CDC change is complete from the Sequin side only after `aura-historia-worker` fans it out to every relevant bounded in-memory sub-worker queue.

Target flow:

1. Postgres commit creates/updates/deletes business rows.
2. Sequin sends CDC event to `aura-historia-worker`.
3. Worker validates source/table/operation and maps the change to domain jobs.
4. Worker derives domain-first idempotency and ordering keys.
5. Worker pushes every derived domain job to all relevant bounded in-memory queues.
6. Worker acknowledges Sequin after all enqueue operations succeed.
7. Sub-workers own retry/backoff and dead-letter behavior.
8. Sub-workers record processed-job markers or dead letters where durable visibility is needed.

Crash rule:

- Crash before Sequin ack: Sequin redelivers.
- Crash after Sequin ack but before sub-worker completion: in-memory jobs may be lost.
- Lost projection jobs are repaired by rebuild/backfill because OpenSearch is rebuildable.
- Jobs with user-visible side effects must make their first external effect idempotent and durable, such as inserting a DynamoDB notification keyed by domain origin event.

This accepts process-memory queues as the first fanout boundary for lower cost and simpler local tests. It requires repair/backfill tooling for any side effect that cannot be safely recomputed.

## Domain job rule

Sub-workers consume domain jobs, not raw infrastructure payloads.

Examples:

- `ProductEventJob { event_id, product_id, event_type }`
- `ShopChangedJob { shop_id, version }`
- `SearchFilterChangedJob { user_id, user_search_filter_id, version, op }`
- `UserTierChangedJob { user_id, version }`
- `PeriodicMatcherJob`

Raw Sequin envelopes stay at the router edge.

## Worker sub-workers

| Sub-worker | Source | Side effects |
|---|---|---|
| Product OpenSearch projector | `product_events` | OpenSearch product upsert/update/delete. |
| Product delete cleanup | `LIFECYCLE_DELETED` event | OpenSearch delete, Postgres watchlist/match cleanup. |
| Watchlist notification generator | price/state product events | DynamoDB notification insert. |
| Search-filter percolator | domain/enrichment product events | Postgres matches, DynamoDB notifications. |
| Product embed | `DOMAIN_CREATED` event | Postgres enrichment event + product row update. |
| Product translate | `ENRICHMENT_EMBEDDED` event | Postgres enrichment event + product row update. |
| Shop OpenSearch projector | shop changes | OpenSearch shop upsert. |
| Search-filter OpenSearch sync | search-filter changes | OpenSearch percolator document upsert/delete. |
| User tier enforcement | user tier changes | Postgres watchlist/search-filter state updates. |
| Periodic matcher | internal schedule | OpenSearch product search, Postgres matches, DynamoDB notifications. |

## Idempotency and ordering

Prefer domain IDs or domain versions over technology-specific CDC IDs.

Minimum rules:

- Product events use `product_events.event_id`.
- Product materialized writes use `products.event_id`.
- Product workers use `event_id` as the idempotency key and `product_id` as the ordering key.
- Shop workers use `(shop_id, version, op)` where version changes on each shop mutation.
- Search-filter workers use `(user_search_filter_id, version, op)`.
- User tier workers use `(user_id, version)`.
- Search-filter matches have a unique key preventing duplicate user/filter/product matches.
- Notifications are idempotent by user and domain origin event where domain allows it.
- `worker_processed_jobs.worker_name + idempotency_key` is unique where durable processed markers are useful.

Use Sequin IDs, LSNs, or raw envelope IDs only as observability fields or last-resort fallback. Do not couple core idempotency to Sequin where a stable domain key exists.

Ordering:

- Product writes serialize per product row in Postgres.
- Worker should route product event jobs with a per-product ordering key.
- Cross-product ordering is not required.

## Testing strategy

- Prefer fast Postgres integration tests over full LocalStack CloudFormation tests.
- Use existing LocalStack OpenSearch support for projection/percolator tests and mapping bootstrap.
- Keep DynamoDB test helpers for notification/OAuth/access-token/FX survivor tests.
- Keep CDK/CloudFormation acceptance deployment ability for infra/survivor wiring.
- Move most REST acceptance coverage to `aura-historia-api` + Postgres + targeted LocalStack services.

## Operations minimum

Postgres becomes business truth. Before production cutover, add:

- tested backup and restore path
- point-in-time recovery or explicit accepted RPO
- WAL retention policy
- Sequin replication slot lag monitoring
- database connection monitoring
- schema migration rollout/rollback runbook
- worker queue lag, retry, and dead-letter alerts
- rebuild/backfill runbooks for OpenSearch and worker-derived side effects

## Deployment target

Default single-host process set:

- Postgres
- Sequin
- OpenSearch
- `aura-historia-api`
- `aura-historia-worker`

All endpoints are env-driven. Postgres and OpenSearch may move to dedicated hosts without code changes.

CloudFront fronts the API. The Hetzner origin must use TLS and origin protection, such as a secret origin header checked by `aura-historia-api`.

## Open questions

- Exact migration/backfill need for pre-production data.
- Exact Sequin delivery mode and auth for worker endpoint.
- Exact deployment tool: Docker Compose, systemd, Terraform, Ansible, or mix.
- Metrics sink for Hetzner-hosted logs/metrics.
