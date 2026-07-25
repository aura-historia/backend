---
name: add-backend-lambda
description: Use only when adding or changing an AWS-survivor Lambda in the Aura Historia backend. Prefer `aura-historia-api` for REST work and `aura-historia-worker` for migrated async business work. Covers survivor boundaries, handler/bootstrap shape, infra wiring, LocalStack/test wiring, docs, and validation.
---

# Add Backend Lambda

Use this skill only for AWS Lambda work that remains inside the hybrid Hetzner/AWS target.

Do **not** add a Lambda for migrated business flows. Prefer:

- `aura-historia-api` for REST routes. Use `add-rest-api-endpoint`.
- `aura-historia-worker` for async business jobs driven by Postgres/Sequin.
- Postgres repositories for migrated business state.

## First checks

- Read the required `AGENTS.md` chain: repo root, `src/AGENTS.md`, `infra/AGENTS.md`, and the nearest crate docs.
- Read target architecture docs when the change touches storage or events:
  - `docs/hetzner_postgres_sequin_migration.md`
  - `docs/storage.md`
  - `docs/events/flow.md`
- Confirm this is an AWS survivor Lambda before editing.
- Find similar Lambdas before designing. Good samples:
  - SQS worker: `src/product-lambda/src/product-lambda-ingest-partner-products/`
  - EventBridge worker: `src/stripe-lambda/`
  - Infra maps: `infra/src/constructs/lambdas.ts`, `queues.ts`, `eventing.ts`
- Decide trigger and retry contract first: SQS, EventBridge partner bus, DynamoDB stream via EventBridge+SQS, schedule, or direct invoke.

## Allowed Lambda scope

Allowed new/changed Lambda work should fit one of these survivor areas:

- notification insert-to-send workflow using DynamoDB + `notification-send` + SES/S3 templates
- Shopify AWS intake, writing product rows/events to Postgres synchronously
- Stripe AWS intake, writing user/subscription state to Postgres synchronously
- Step Functions task Lambda for partner-shop-application workflow, writing Postgres directly
- FX rate scheduled Lambda using DynamoDB as survivor storage
- OAuth clients/codes/access-token DynamoDB flows
- Cognito integration that cannot move into `aura-historia-api`
- CloudWatch log-retention utility
- infra/test glue for survivor Lambdas

If the flow is product projection, percolation, user tier enforcement, product enrichment, shop/search-filter OpenSearch sync, or periodic matcher, prefer `aura-historia-worker` unless the user explicitly asks for AWS-survivor behavior.

## Target storage rules

- Postgres owns migrated business truth: users, shops, partner applications, products, product events, watchlist, search filters, and search-filter matches.
- DynamoDB keeps notifications, access tokens, OAuth clients/codes, and FX rate.
- OpenSearch is rebuildable projection only.
- No users OpenSearch index in target design.
- No Postgres `access_tokens` or `access_token_scopes`.
- No worker-owned Postgres tables: no `worker_inbox`, `worker_processed_jobs`, `worker_dead_letters`, or `worker_schedules`.
- No scheduled inconsistency checker or repair job for MVP.
- Crawler is out of scope.

AWS survivor Lambdas that need migrated business data connect to Postgres directly. Do not call internal `aura-historia-api` as a data bridge.

## Choose crate location

- Prefer an existing grouped parent when the worker belongs to an existing survivor domain:
  - child crate path: `src/<domain>-lambda/src/<binary-name>/`
  - examples: `product-lambda-*`, `search-filter-lambda-*`, `user-lambda-*`
- Use a top-level `src/<name>-lambda/` crate only when no existing parent fits or the Lambda is a standalone integration.
- Name the Cargo package and deployed binary in kebab-case, e.g. `shopify-lambda-sync-product`.
- Add or update an `AGENTS.md` at the crate root. For child crates, also update the parent Lambda `AGENTS.md` child index.
- Do not create new API Gateway adapter Lambdas for migrated API routes.

## Rust workspace wiring

Update only the files the new crate needs:

- `Cargo.toml`
  - add the crate to `[workspace.members]`
  - add a `[workspace.dependencies]` path entry if other crates refer to it by workspace dependency
  - add package dependencies only where the existing pattern requires it
- Parent Lambda crate, when using a grouped parent:
  - add the child crate to `src/<domain>-lambda/Cargo.toml`
  - add a `pub use <crate_name>;` in `src/<domain>-lambda/src/lib.rs` when the parent re-exports children
- New crate `Cargo.toml`
  - use workspace dependencies
  - enable event features explicitly, e.g. `aws_lambda_events = { workspace = true, features = ["sqs"] }`
  - use `common::postgres` / existing Postgres helpers when writing migrated business data
  - keep dev-dependencies targeted: `fake`, `rstest`, `serial_test`, `test-api`, and domain `test-data` features only when needed

## Handler shape

- Keep `src/main.rs` as bootstrap only:
  - `common::logging::init_logging()` first
  - load AWS config with the repo's current `BehaviorVersion`
  - load env vars once with clear names matching infra
  - build repositories/services
  - log `debug!("Lambda initialized.")`
  - call `lambda_runtime::run(service_fn(...))`
- Put testable behavior in `src/lib.rs` or small service modules.
- Add `#[tracing::instrument(skip(...), fields(requestId = %event.context.request_id, ...))]` on handlers.
- Keep logs compact and structured. Do not log PII, secrets, raw tokens, or full payloads unless the payload is known safe.
- Do not hide business rules in Lambda glue. Put reusable behavior into the domain/service crate.
- When writing Postgres, use repositories/DAOs. Do not operate directly on SQL rows outside adapter boundaries.

## Postgres access from AWS Lambda

When a survivor Lambda writes or reads migrated business data:

- Use TLS to Postgres.
- Load credentials from AWS SSM/Secrets Manager or equivalent secret source.
- Keep Lambda connection pools very small.
- Add PgBouncer or equivalent if concurrency can exceed safe Postgres connections.
- Restrict network path by allowlist, VPN, tunnel, private overlay, or another reviewed control.
- Monitor failed connects, connection count, pool wait, and auth failures.
- Product writes must synchronously insert `product_events` and update `products` in one transaction. No product command buffer and no `202 accepted because queued` semantics.

## Event and retry rules

### SQS workers

- Use `aws_lambda_events::sqs::{SqsEvent, SqsBatchResponse, BatchItemFailure}`.
- If infra sets `reportBatchItemFailures: true`, return only failed message IDs so good records are not retried.
- Treat invalid poison messages deliberately:
  - return a batch item failure if it should go to the DLQ
  - log and skip only when retry cannot help and the event is safe to drop
- Keep handlers idempotent. Assume duplicate delivery.
- Prefer domain IDs for idempotency. Avoid coupling business idempotency to SQS message IDs.
- Pick batch size, concurrency, and visibility timeout together. Visibility timeout must exceed Lambda timeout; heavy queue workers usually use about `6x` timeout unless there is a reason not to.

### EventBridge workers

- Parse the envelope narrowly. Match supported `detail.type` or stable event fields.
- Unknown unsupported event types should usually be `warn` + `Ok(())`.
- Return an error only for retryable failures.
- Document source bus, event pattern, side effects, and idempotency.

### DynamoDB stream workers

- Only use for survivor DynamoDB flows, such as notification send.
- Do not add DynamoDB streams for migrated Postgres business tables.
- Keep payload mapping at the edge. Pass domain types into reusable services.

### Scheduled workers

- Keep schedules in `infra/src/constructs/eventing.ts` or a focused construct.
- Fail fast. Avoid long timeout camping.
- Skip ephemeral stage when the job depends on real third-party state.
- Do not add a scheduled inconsistency checker/repair job for the worker MVP unless the architecture decision changes.

## Infra wiring

Update `infra/` in the same change when adding trigger, env var, IAM, queue, route, or schedule.

- `infra/src/constructs/lambdas.ts`
  - add a `LAMBDA_DEFINITIONS` entry with stable `id`, `binaryName`, memory, timeout, and environment
  - include stage-specific secrets via SSM helpers; use test values for ephemeral when needed
  - add Postgres env/secret wiring when the survivor Lambda writes migrated business data
  - update `grantRuntimeAccess` lists for DynamoDB, OpenSearch, S3/SES, queue send, or other resource access
  - update `addUserPoolEnvironment` / `grantCognitoAdminAccess` if Cognito is used
  - keep CloudWatch log-retention utility wiring
- `infra/src/constructs/queues.ts` for SQS-backed workers:
  - add main queue + DLQ definition
  - choose `maxReceiveCount`, `visibilityTimeoutSeconds`, and SSE intentionally
- `infra/src/constructs/eventing.ts`
  - add EventBridge rule, DynamoDB stream rule, schedule, or SQS event source
  - set `batchSize`, `reportBatchItemFailures`, and optional batching window
  - add queue policies when EventBridge targets SQS
- `infra/src/application-stack.ts`
  - add CloudFormation outputs only when tests, users, or tooling really need them
- Run or update infra tests when stack shape changes.

Do not add API Gateway routes for migrated API work. CloudFront fronts `aura-historia-api`, not new API Lambdas.

## Test and LocalStack wiring

- Unit-test handler behavior with mocks. Cover happy path, invalid payload, downstream error, retry/drop decision, and idempotency edge cases.
- Add integration tests for repository or AWS adapter code touched by the Lambda.
- Use `test-api` Postgres helpers for Postgres repositories or direct Lambda-to-Postgres integration tests.
- Use existing LocalStack OpenSearch for OpenSearch tests. Do not add a standalone OpenSearch container.
- Keep DynamoDB and CDK/CloudFormation helpers for survivor tests.
- Add acceptance tests for critical survivor event flows. Every survivor Lambda should be invoked indirectly downstream in at least one acceptance test when practical.
- Update LocalStack/cloud wiring when the Lambda participates in deployed tests:
  - `src/test-api/src/cloudformation.rs` `LAMBDA_BINARIES`
  - queue drain list in `src/test-api/src/cloudformation.rs` when a queue is added
  - `src/aws-tests/src/aws-tests-common/src/lib.rs` `CloudFormationOutput` when adding outputs
  - acceptance-test reset helpers when they drain or inspect the new resource
- Update `src/ci-determinator/src/main.rs` `INTEGRATION_TEST_CRATES` if the new crate has integration tests run in CI.

## Docs and DOX

Update docs in the same change when behavior changes:

- nearest crate `AGENTS.md`: purpose, modules, env vars, event shape, side effects, verification
- parent `AGENTS.md` child index for new child crates
- `src/AGENTS.md` child index for new top-level crates
- `docs/events/flow.md` for survivor event-flow changes
- `docs/storage.md` for storage ownership changes
- `docs/dynamodb/table_1.md` for DynamoDB survivor structure changes
- OpenSearch mappings under `opensearch/mappings` if DTO/index shape changes
- infra docs if deployment, env vars, or IAM shape changes

## Validation

Start narrow, then grow:

1. `cargo check -p <new-crate>`
2. `cargo test -p <new-crate> --all-features`
3. If parent/domain wiring changed: `cargo check -p <parent-or-domain-crate>`
4. If infra changed: `npm --prefix infra test` and relevant synth (`npm --prefix infra run synth -- --context stage=ephemeral` or `npm --prefix infra run synth:all`)
5. For broad changes: `cargo check --workspace`

Report skipped heavy LocalStack/acceptance tests clearly.
