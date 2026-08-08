# DOX

## Purpose

- Own `test-api` crate.

## Core Design

- LocalStack and AWS integration test harness.
- Root modules: `api_gateway`, `aura_historia_api`, `cloudformation`, `cognito`, `dynamodb`, `eventbridge`, `localstack`, `opensearch`, `postgres`, `s3`, `sequin`, `ses`, `signal`, `sqs`.
- Child crates: `test-api-macros`.
- Main neighbors: `aws-tests-common`, `common`, `test-api-macros`, `user`.
- Test crate. Favor stable helpers and black-box assertions.
- `#[aura_integration_test]` tests run serially inside one test process against process-local LocalStack and optional service containers like Postgres.
- Postgres reapplies migrations before each test to restore migration seed data, then truncates data between tests. Use `Postgres::new_schema_once` only for schema-only migrations; optional setup scripts always run before each test.
- LocalStack and Postgres use process-id-scoped container names and host ports so separate test binaries/processes can run in parallel.
- OpenSearch setup uses one canonical `user_search_filters` index.

## Ownership

- This doc rule `src/test-api/**`.
- Parent doc: `src/AGENTS.md`.
- Child docs below rule deeper child crates.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, route/event shape, env vars, or child index change.
- Keep fixtures deterministic. Add or move suite paths in `src/ci-determinator` when CI scope change.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Tests prove behavior, not implementation trivia.
- Share helpers before copy-paste fixtures.
- Prefer `Postgres`/`OperationalBackendPostgres` and `postgres` feature over legacy `Rds`/`rds` in new tests.
- Use process-lived `AuraHistoriaApi` helper for local black-box tests against `aura-historia-api`.
- Use `Postgres::new("migrations")` for the shared business schema.
- Use `Sequin::worker_webhook()` in `#[aura_integration_test]` plus `get_sequin_worker_webhook_bind_addr()` when a test must verify real Sequin webhook delivery to a local worker endpoint. Its test sink sends insert, update, and delete changes for `product_events` and `search_filters`.

## Verification

- `cargo check -p test-api`
- `cargo test -p test-api --all-features`

## Child DOX Index

- `src/test-api/src/test-api-macros/AGENTS.md` — `test-api-macros` crate.
