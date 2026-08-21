# DOX

## Purpose

- Own bare-metal async worker runtime, Sequin CDC ingestion, and in-memory worker queues for #1341.

## Core Design

- `main.rs` reads `LOG_LEVEL`, bootstraps typed `platform-observability` logging, config, health/CDC server, and graceful shutdown.
- `lib.rs` owns runtime config including typed worker-local `POSTGRES_*` parsing, `/health`, `/ready`, `/cdc/sequin`, server loop, default all-queue runtime, and bounded queue primitives. Runtime wiring imports direct owners: `platform-postgres` SQLx mechanics, `platform-observability` logging, `application` contracts/errors, and bounded-context core values; it has no `common` dependency.
- `cdc.rs` normalizes Sequin webhook JSON to domain jobs and fans out after route validation.
- `product_opensearch.rs` consumes `ProductOpenSearch` jobs, rereads the committed current Product source by stable event/Product IDs, loads an immutable sale snapshot only when a main source price needs sale-time conversion, then writes or deletes the canonical rebuildable Product document with OpenSearch external version protection from `products.projection_version`.
- `search_filter_projection.rs` consumes `SearchFilterOpenSearch` jobs, rereads committed Postgres state, and writes its FX-independent canonical OpenSearch projection with target-side source-version protection.
- `search_filter_percolator.rs` maps domain/enrichment `ProductEvent` jobs to the inbound matching command and invokes the service. The service rereads committed Product state with its exact persisted sale FX snapshot, skips stale triggers whose event ID no longer matches current state, then locks/rechecks the Product revision in the final Postgres match transaction before canonical match persistence. It evaluates enhanced filters outside Postgres. No legacy evaluator is linked.
- `search_filter_match_notifications.rs` maps persisted-match insert jobs to the inbound notification command and invokes the service. Match jobs and source reads use `(user_id, user_search_filter_id, product_id, origin_event_id)`; stale or superseded rows suppress without notification. The service creates one idempotent PostgreSQL SearchFilter notification per matching filter after source validation. Worker logs distinguish inserted notifications from semantic duplicates.
- `watchlist_notifications.rs` consumes price/state Product event jobs, rereads committed Postgres source, locks and rechecks the Product revision through notification commit, selects recipients using `product_events.event_time` and current watchlist intervals, then creates idempotent PostgreSQL notification records. Its worker acknowledges and logs `Applied`, duplicate, missing-source suppression, and stale-event suppression separately; only actual failures retry or enter the in-memory DLQ.
- `notification_delivery.rs` consumes notification-delivery insert jobs and invokes the canonical delivery use case. The use case claims the durable PostgreSQL delivery lease, dispatches by persisted channel once per claimed attempt, captures the provider outcome, then retries only the matching terminal PostgreSQL update with the original lease token, completion timestamp, and provider receipt/error code before reporting completion. EMAIL currently uses S3 templates and SES; delivery source reads carry current prohibited-content consent so unsafe notification image URLs are omitted before rendering.
- `product_embedding.rs` consumes `DOMAIN_CREATED` Product event jobs, rereads committed current Product state, invokes the neutral Vertex embedding capability before a short transaction, and atomically persists the vector plus `ENRICHMENT_EMBEDDED`. Stale and duplicate jobs are explicit no-ops.
- `product_translation.rs` consumes `ENRICHMENT_EMBEDDED` Product event jobs, uses `localization::Language`, rereads the committed current Product source, invokes the configured neutral Vertex LLM title translator, and atomically persists canonical Postgres translations plus one translated-titles enrichment event. Stale and duplicate jobs are explicit no-ops.
- `retry.rs` owns in-process retry, idempotency memory, and in-memory DLQ helpers.
- No worker persistence tables in MVP. Crash after CDC fan-out may lose queued jobs; the post-ack loss window is tracked by #1558.

## Ownership

- This doc rule `src/aura-historia-worker/**`.
- Parent doc: `src/AGENTS.md`.

## Local Contracts

- Read repo root, `src/AGENTS.md`, then here before edit.
- Update this doc when env vars, queue behavior, dependencies, or runtime behavior changes.
- Runtime scope comes from required `AURA_HISTORIA_WORKER_SCOPE`: `search-filter-projection` accepts only `search_filters`; `search-filter-percolator` accepts only domain/enrichment `product_events` inserts; `search-filter-match-notification` accepts only `search_filter_matches` inserts; `watchlist-notification` accepts only canonical price/state `product_events` inserts; `notification-delivery` accepts only `notification_deliveries` inserts; `product-embedding` accepts only `DOMAIN_CREATED` `product_events` inserts; `product-translation` accepts only `ENRICHMENT_EMBEDDED` `product_events` inserts; `product-opensearch` accepts only `product_events` inserts. Only `STAGE=ephemeral`, `local`, or `test` may default to projection. Other tables fail delivery rather than filling unconsumed queues. Configure each Sequin subscription accordingly.
- Startup validates scoped configuration before Postgres connects or readiness binds. Every scope requires `POSTGRES_*`. Search-filter projection/percolator scopes require `OPENSEARCH_ENDPOINT_URL` and, outside local development, OpenSearch credentials. Percolator and product-translation require explicit `VERTEX_AI_PROJECT_ID`, `VERTEX_AI_LOCATION`, `VERTEX_AI_MODEL`, Google ADC with Cloud Platform scope, and a buildable configured large-language-model Vertex HTTP client; product-embedding requires only `VERTEX_AI_PROJECT_ID`, `VERTEX_AI_LOCATION`, and Google ADC. Match-notification, watchlist, and notification-delivery scopes use PostgreSQL notification adapters and do not load DynamoDB configuration. Generic delivery config owns optional channel configs. Current EMAIL config needs `S3_BUCKET_NAME_TEMPLATES`, `NOTIFICATION_EMAIL_FROM`, `NOTIFICATION_EMAIL_REPLY_TO`, `STAGE`, `COMMIT_SHA`, and AWS credentials with S3 template-read plus SES send permission. Channel-specific runtime wiring resolves EMAIL targets; startup registers configured senders and verifies all production planner channels have a sender. Only the selected scope initializes its OpenSearch, Vertex, S3/SES, and source-reader adapters.
- Event-flow changes must update `docs/events/flow.md`.

## Work Guidance

- Keep runtime glue thin.
- Worker implements no service port or use case. It only maps CDC/queue jobs and composes adapters from adapter crates. Runtime-local queue and transport traits are allowed.
- Register all known worker queues by default when every route has a consumer. A dedicated runtime may register only its explicitly scoped CDC route; it must reject other tables before acknowledgment.
- Queue payloads should be domain types or domain IDs, not Sequin/AWS envelopes.
- Sub-worker implementation must extract typed DTOs/payloads from Postgres/domain rows when behavior needs event/change fields; do not consume raw Sequin JSON outside router.
- Ack Sequin only after all relevant bounded queue enqueues succeed.
- Use domain idempotency keys; Sequin IDs/LSNs are logs only.
- Keep queue abstraction replaceable by SQS/Lambda/ECS later.
- Every production worker route needs rigorous black-box acceptance tests in `tests/` using real Postgres, Sequin, the running worker server, and every written target store. Cover happy path, rollback, ignored changes, redelivery/idempotency, filtering, and persisted output shape. Notification delivery tests use LocalStack S3 templates plus SES and assert persisted delivery state. Declare `Sequin::worker_webhook()` after fixtures in `#[aura_integration_test]`; it owns one process-lived subscription. Worker helpers bind `get_sequin_worker_webhook_bind_addr()` and own only runtime shutdown. Start the worker before writing watched source rows; fixture helpers must not emit those rows.

## Verification

- `cargo check -p aura-historia-worker`
- `cargo test -p aura-historia-worker --all-features`

## Child DOX Index

- None.
