---
name: add-backend-lambda
description: Use when adding a new AWS Lambda to the Aura Historia Rust backend. Covers crate placement, handler/bootstrap shape, infra wiring, queue/EventBridge/API triggers, LocalStack/test wiring, docs, and validation.
---

# Add Backend Lambda

Use this skill when the task adds a new Lambda binary or a new Lambda worker flow.
If the task is only adding a route to an existing `*-api` Lambda, use `add-rest-api-endpoint` instead.

## First checks

- Read the required `AGENTS.md` chain: repo root, `src/AGENTS.md`, `infra/AGENTS.md`, and the nearest crate docs.
- Find similar Lambdas before designing. Good samples:
  - API Lambda bootstrap: `src/product-api/src/main.rs`
  - SQS worker: `src/product-lambda/src/product-lambda-ingest-partner-products/`
  - EventBridge worker: `src/stripe-lambda/`
  - Infra maps: `infra/src/constructs/lambdas.ts`, `queues.ts`, `eventing.ts`
- Decide the trigger and retry contract first: API Gateway, SQS, EventBridge partner bus, DynamoDB stream via EventBridge+SQS, schedule, or direct invoke.

## Choose crate location

- Prefer an existing grouped parent when the worker belongs to an existing domain:
  - child crate path: `src/<domain>-lambda/src/<binary-name>/`
  - examples: `product-lambda-*`, `search-filter-lambda-*`, `user-lambda-*`
- Use a top-level `src/<name>-lambda/` crate only when no existing parent fits or the Lambda is a standalone integration.
- Name the Cargo package and deployed binary in kebab-case, e.g. `product-lambda-rebuild-foo`.
- Add or update an `AGENTS.md` at the crate root. For child crates, also update the parent Lambda `AGENTS.md` child index.

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

## Event and retry rules

### SQS workers

- Use `aws_lambda_events::sqs::{SqsEvent, SqsBatchResponse, BatchItemFailure}`.
- If infra sets `reportBatchItemFailures: true`, return only failed message IDs so good records are not retried.
- Treat invalid poison messages deliberately:
  - return a batch item failure if it should go to the DLQ
  - log and skip only when retry cannot help and the event is safe to drop
- Keep handlers idempotent. Assume duplicate delivery.
- Pick batch size, concurrency, and visibility timeout together. Visibility timeout must exceed Lambda timeout; heavy queue workers usually use about `6x` timeout unless there is a reason not to.

### EventBridge workers

- Parse the envelope narrowly. Match supported `detail.type` or stable event fields.
- Unknown unsupported event types should usually be `warn` + `Ok(())`.
- Return an error only for retryable failures.
- Document source bus, event pattern, side effects, and idempotency.

### Scheduled workers

- Keep schedules in `infra/src/constructs/eventing.ts` or a focused construct.
- Fail fast. Avoid long timeout camping.
- Skip ephemeral stage when the job depends on real third-party state.

## Infra wiring

Update `infra/` in the same change when adding trigger, env var, IAM, queue, route, or schedule.

- `infra/src/constructs/lambdas.ts`
  - add a `LAMBDA_DEFINITIONS` entry with stable `id`, `binaryName`, memory, timeout, and environment
  - include stage-specific secrets via SSM helpers; use test values for ephemeral when needed
  - update `grantRuntimeAccess` lists for DynamoDB (default), OpenSearch, S3/SES, queue send, or other resource access
  - update `addUserPoolEnvironment` / `grantCognitoAdminAccess` if Cognito is used
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

## Test and LocalStack wiring

- Unit-test handler behavior with mocks. Cover happy path, invalid payload, downstream error, retry/drop decision, and idempotency edge cases.
- Add integration tests for repository or AWS adapter code touched by the Lambda.
- Add acceptance tests for critical event flows. Every lambda should be invoked (indirectly downstream most often) in at least one acceptance test.
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
- `docs/events/flow.md` for event-driven flow changes
- `docs/dynamodb/table_1.md` for DynamoDB structure changes
- OpenSearch mappings under `opensearch/mappings` if DTO/index shape changes

## Validation

Start narrow, then grow:

1. `cargo check -p <new-crate>`
2. `cargo test -p <new-crate> --all-features`
3. If parent/domain wiring changed: `cargo check -p <parent-or-domain-crate>`
4. If infra changed: `npm --prefix infra test` and relevant synth (`npm --prefix infra run synth -- --context stage=ephemeral` or `npm --prefix infra run synth:all`)
5. For broad changes: `cargo check --workspace`

Report skipped heavy LocalStack/acceptance tests clearly.
