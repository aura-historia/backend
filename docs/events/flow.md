# Event Flow

This document describes the target event flow for #1341. Current AWS DynamoDB Stream/EventBridge/SQS/Lambda rails stay only for AWS survivor workflows until cutover.

See `docs/hetzner_postgres_sequin_migration.md` for the ADR.

## Target components

| Component | Type | Purpose |
|---|---|---|
| Postgres | Database | Business source of truth and transactional product/event writes. |
| `product_events` | Postgres table | Product domain/enrichment/policy/lifecycle event log. |
| Sequin | CDC | Delivers committed Postgres changes to worker ingestion. |
| `worker_inbox` | Postgres table | Durable CDC/job inbox before in-memory queues. |
| `aura-historia-worker` router | Rust process | Maps CDC/inbox rows to domain jobs. |
| In-memory sub-worker queues | Worker buffers | Bounded execution buffers, not durability boundary. |
| OpenSearch | Search projection | Rebuildable product/shop/search-filter projection. |
| DynamoDB notifications | AWS DynamoDB | Notification TTL and insert-to-send behavior. |
| `notification-send` | AWS Lambda | Sends external notifications through SES. |
| FxRate Lambda | AWS Lambda | Updates FX rates in DynamoDB. |
| Shopify Lambda | AWS Lambda | Handles Shopify events, writes Postgres directly. |
| Stripe Lambda | AWS Lambda | Handles Stripe subscription events, writes Postgres directly. |
| Step Functions | AWS workflow | Partner-shop-application workflow. |
| CloudWatch log-retention Lambda | AWS Lambda | Keeps AWS log retention policy. |

## Target routing diagram

```mermaid
flowchart TD
    API["aura-historia-api"]
    SHOPIFY["Shopify Lambda"]
    STRIPE["Stripe Lambda"]
    SFN["Step Functions Lambda"]
    PG[(Postgres)]
    SEQ["Sequin CDC"]
    INBOX[(worker_inbox)]
    ROUTER["aura-historia-worker router"]
    OS[(OpenSearch)]
    DDBN[(DynamoDB notifications)]
    SEND["notification-send Lambda"]
    SES["SES"]
    FX["FxRate Lambda"]
    DDBFX[(DynamoDB FX rate)]

    API -->|"sync business transaction"| PG
    SHOPIFY -->|"sync product/event transaction"| PG
    STRIPE -->|"sync user update"| PG
    SFN -->|"sync partner-app/shop/user update"| PG

    PG -->|"committed row changes"| SEQ
    SEQ -->|"deliver CDC"| INBOX
    INBOX --> ROUTER

    ROUTER -->|"product projection jobs"| OS
    ROUTER -->|"shop projection jobs"| OS
    ROUTER -->|"search-filter percolator docs"| OS
    ROUTER -->|"match/watchlist/tier updates"| PG
    ROUTER -->|"notification records"| DDBN
    ROUTER -->|"enrichment events + product update"| PG

    DDBN -->|"stream/event rule"| SEND
    SEND --> SES

    FX --> DDBFX
```

## Product write flow

Product writes are synchronous.

```mermaid
sequenceDiagram
    participant Caller
    participant API as aura-historia-api or AWS intake Lambda
    participant PG as Postgres
    participant Sequin
    participant Worker as aura-historia-worker
    participant OS as OpenSearch

    Caller->>API: product create/update/delete
    API->>PG: begin transaction
    API->>PG: lock/read product row
    API->>PG: insert product_events
    API->>PG: upsert/update products
    API->>PG: commit
    API-->>Caller: success/failure after commit
    PG-->>Sequin: CDC after commit
    Sequin->>Worker: deliver CDC
    Worker->>PG: insert worker_inbox idempotently
    Worker-->>Sequin: ack after durable write
    Worker->>OS: project product/search side effects
```

No intermediate product command SQS queue. No `202 accepted because queued` behavior for migrated writes.

## Durable worker inbox

Sequin delivery is not processed directly in memory.

Minimum ingest steps:

1. Receive CDC envelope.
2. Validate source/table/operation.
3. Derive stable `source_event_id`, `aggregate_type`, `aggregate_id`, and `ordering_key`.
4. Insert into `worker_inbox` with unique `(source, source_event_id)`.
5. Ack Sequin only after insert succeeds or duplicate is detected.
6. Router leases pending inbox rows and pushes domain jobs to bounded in-memory queues.
7. Sub-worker records processed idempotency key or failure.

Crash rule:

- Crash before inbox insert: Sequin redelivers.
- Crash after inbox insert before ack: duplicate delivery becomes no-op insert.
- Crash after enqueue: inbox row remains retryable unless processed marker exists.

## CDC routing draft

| Source table | Operation | Route |
|---|---|---|
| `product_events` | INSERT | Product projector; percolator for domain/enrichment; watchlist notifications for price/state; enrichment pipeline for create/embed; delete cleanup for lifecycle delete. |
| `products` | INSERT/MODIFY | No default downstream route. Product events are the projection trigger to avoid double-firing. Use products CDC only for explicit backfills or future non-event projections. |
| `shops` / `shop_domains` | INSERT/MODIFY/DELETE | Shop OpenSearch projector. |
| `search_filters` | INSERT/MODIFY/DELETE | Search-filter OpenSearch sync; user tier checks if state/feature-relevant. |
| `search_filter_matches` | INSERT/MODIFY/DELETE | Usually no downstream route except observability/backfill. |
| `users` | INSERT/MODIFY/DELETE | User tier enforcement for tier changes; no user OpenSearch projection. |
| `product_watchlist` | INSERT/MODIFY/DELETE | Usually no downstream route except tier/backfill; product events drive notifications. |
| `partner_shop_applications` | INSERT/MODIFY | No generic worker route unless notification/backfill requires it. |

## Domain jobs

Worker sub-jobs use domain payloads or compact IDs. They do not use DynamoDB stream records and should not depend on raw Sequin JSON outside the router.

Examples:

- `ProductEventJob`
- `ProductDeletedJob`
- `SearchFilterChangedJob`
- `ShopChangedJob`
- `UserTierChangedJob`
- `PeriodicMatcherJob`

## Target sub-workers

| Sub-worker | Replaces | Input | Side effects |
|---|---|---|---|
| Product OpenSearch projector | `product-lambda-materialize-opensearch` | Product event job | OpenSearch product document create/update/delete. |
| Product delete cleanup | `product-lambda-delete-product` | Lifecycle deleted job | OpenSearch delete, Postgres watchlist/match cleanup. |
| Watchlist notification generator | `product-lambda-update-notify-user` | Price/state product event job | DynamoDB notification inserts. |
| Search-filter percolator | `search-filter-lambda-percolate-product` | Domain/enrichment product event job | Postgres matches, DynamoDB notifications. |
| Product embed | `product-pipeline-embed-text` | Domain created job | Postgres enrichment event + product update. |
| Product translate | `product-pipeline-translate` | Enrichment embedded job | Postgres enrichment event + product update. |
| Shop OpenSearch projector | `shop-lambda-opensearch-index` | Shop changed job | OpenSearch shop document write. |
| Search-filter OpenSearch sync | `search-filter-lambda-opensearch-sync` | Search-filter changed job | OpenSearch percolator document write/delete. |
| User tier enforcement | `user-lambda-tier-update` | User tier changed job | Postgres watchlist/search-filter state updates. |
| Periodic matcher | ECS periodic matcher | Scheduled job | OpenSearch product search, Postgres matches, DynamoDB notifications. |

## AWS survivor event flow

These AWS event flows stay:

| Source | Route | Target |
|---|---|---|
| DynamoDB notification insert | Stream/EventBridge/SQS | `notification-send` Lambda |
| EventBridge schedule | cron | `fxrate-lambda` |
| Shopify partner EventBridge/SQS | Shopify product events | `shopify-lambda`; this is external intake buffering before sync Postgres product/event writes, not the removed product command queue. |
| Stripe partner EventBridge | subscription events | `stripe-lambda` |
| Step Functions | partner app workflow | `partner-shop-application-lambda`; Lambda writes Postgres business rows directly. |
| CloudWatch log group events | EventBridge | CloudWatch log-retention Lambda |

## Idempotency

Minimum unique keys:

| Area | Key |
|---|---|
| Product event | `product_events.event_id` |
| Worker inbox | `(source, source_event_id)`, where `source_event_id` is Sequin event ID or derived from `lsn + table + primary key + operation` |
| Worker processed | `(worker_name, idempotency_key)` |
| Search-filter match | `(user_search_filter_id, product_id)` plus user/filter/product lookup indexes |
| Notification | user + origin event where domain allows |

External sends remain at-least-once. Notification duplicate protection is at record creation, not SES delivery.

## Retry and poison handling

Each inbox row has:

- `status`
- `attempts`
- `next_attempt_at`
- `locked_by`
- `locked_until`

Sub-workers retry transient failures with backoff. Permanent failures move to `worker_failures` and keep enough payload to debug/replay.

## Operations notes

Postgres is business truth and Sequin depends on replication health. Production cutover needs backup/restore, WAL/PITR or accepted RPO, Sequin slot lag monitoring, worker inbox lag alerts, poison-count alerts, and Postgres connection monitoring.

## Test guidance

- Use Postgres integration tests for repository and inbox behavior.
- Use fake CDC envelopes for router tests.
- Use existing LocalStack OpenSearch for projection/percolator tests.
- Keep DynamoDB and CDK/CloudFormation helpers for AWS survivor tests.
