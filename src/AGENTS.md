# DOX

## Purpose

- Own Rust workspace map.
- Own `src/lib.rs` and `src/opensearch/`.
- Hold global Rust work rules for all crates under `src/`.

## Core Design

- Workspace split by job: domain libs hold rules, `*-api` crates speak HTTP, `*-lambda` crates speak event/runtime, test crates prove behavior.
- Keep reusable logic in domain or service modules. Handler `main.rs`, route files, and Lambda bootstrap stay thin.
- Shared OpenSearch assets under `src/opensearch/` stay governed here unless they grow own durable boundary.
- Crate submodule-design
  - core: domain logic and business rules
  - data: REST-API payloads
  - dynamodb: DynamoDB payloads
  - opensearch: OpenSearch payloads
  - service: service glue, orchestration, and cross-crate integration
- DynamoDB is primary datastore and source-of-truth, OpenSearch is re-computable read-optimized view, specifically for search and discovery. Kept in sync primarily via event-driven architecture through AWS Event Bridge + SQS + Lambda.
- Cognito is only Identity-Provider. User-Details and User-Profile are stored in DynamoDB.

## Ownership

- This doc rule `src/**`.
- Crate doc rule its crate path.
- Near doc win detail.

## Local Contracts

- Read root, then here, then crate doc, before edit.
- New `src` doc go at crate root. No module doc unless module become crate boundary.
- Update nearest doc when crate purpose, route, event, env var, dependency edge, test flow, or child index change.
- If REST endpoint, payload, auth, or error behavior change, update `docs/swagger.yaml` and `docs/CHANGELOG.md`.
- If relevant DynamoDB structure change, update `docs/dynamodb/table_1.md`
- If OpenSearch DTOs change, make sure the corressponding index-mappings in `opensearch/mappings` are aligned
- If relevant event structure or flow change, update `docs/evenst/flow.md`
- If new Lambda appear, wire deploy, `src/ci-determinator`, `src/test-api/src/cloudformation.rs`, and `infra/` when needed.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Match crate pattern. Keep cross-crate bleed low.
- Prefer targeted package edits and tests first. Grow wider only when change cross crate.
- Use `rstest` when table test fit. Use `fake::Dummy<fake::Faker>` test data when crate support it.
- Test names should say what happen and when, like `should_*_when_*` (optional suffix `_for_*`).
- Prefer functional-style code. 
- Avoid `unsafe` and `unsafe`-like patterns like `unwrap`, `expect`, and `panic!`. Use `Result` and `Option` instead. 
- Never swallow errors (e.g. `.ok()`). Use `?` operator to propagate errors. Use `thiserror` with well-typed error-enums.
- Apply semi-strict Domain-Driven Design (DDD) principles. Keep domain logic in domain modules, and keep service modules thin. Avoid anemic models. Make use of newtypes.
- Types are strictly separated between:
  - REST-API: suffix `Data`, e.g. `LanguageData`
  - DynamoDB: suffix `Record`, e.g. `LanguageRecord`
  - OpenSearch: suffix `Document`, e.g. `LanguageDocument`
  - Domain: no suffix, e.g. `Language`

## Build And Validate

- Fast path: `cargo check -p <crate>`
- Workspace check: `cargo check --workspace`
- Format: `cargo fmt --all -- --check`
- Lint: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Unit tests: `cargo test -p <crate> --all-features` or `cargo test --workspace --lib --all-features`
- LocalStack integration need Docker. Acceptance tests be heavy; run when task asks or risk says so.
- Run integration-tests (tests folder) only for targetted crates. 

## Test Guidance

- Full coverage everywhere required: all happy and unhappy paths, all edge cases, all error cases.
- Use `rstest` for parameterized tests where sensible
- Use `fake` crate for generating test data where applicable
- Use `..Default::default()` for struct initialization in tests to avoid boilerplate
- Unit-test all public functions and critical private functions
- Integration-test all repository-functions (OpenSearch, DynamoDB)
- Integration-test all lambdas for happy paths
- Acceptance-Test all critical application-flows for cross-crate integration and end-to-end behavior. This includes
  - REST API endpoints
  - Lambda event flows
  - Event-driven architecture flows (EventBridge, SQS, Lambda)
  - Critical business rules, edge cases and authentication/user-plan flows

## Runtime Guidance

- Init executable logging with `common::logging::init_logging()`.
- Keep logs compact & structured for CloudWatch-Analysis. Error log mean real fire. Expected failure be `warn` or lower.
- Do not hide business rules in handler glue. Parse, auth, and map in edge crate; real rule live deeper.
- Keep env var names, queue names, and event shapes stable and documented in nearest crate doc.

## Verification

- Whole workspace: `cargo check --workspace`
- Whole unit tests: `cargo test --workspace --lib --all-features`

## Child DOX Index

- `src/acceptance-tests/AGENTS.md` — `acceptance-tests` crate.
- `src/aws-tests/AGENTS.md` — `aws-tests` crate.
- `src/ci-determinator/AGENTS.md` — `ci-determinator` crate.
- `src/cloudwatch-log-retention-lambda/AGENTS.md` — `cloudwatch-log-retention-lambda` crate.
- `src/cognito/AGENTS.md` — `cognito` crate.
- `src/cognito-post-confirmation/AGENTS.md` — `cognito-post-confirmation` crate.
- `src/common/AGENTS.md` — `common` crate.
- `src/crawler/AGENTS.md` — `crawler` crate.
- `src/fxrate/AGENTS.md` — `fxrate` crate.
- `src/fxrate-lambda/AGENTS.md` — `fxrate-lambda` crate.
- `src/geo/AGENTS.md` — `geo` crate.
- `src/newsletter-api/AGENTS.md` — `newsletter-api` crate.
- `src/notification/AGENTS.md` — `notification` crate.
- `src/notification-api/AGENTS.md` — `notification-api` crate.
- `src/notification-send/AGENTS.md` — `notification-send` crate.
- `src/oauth/AGENTS.md` — `oauth` crate.
- `src/oauth-api/AGENTS.md` — `oauth-api` crate.
- `src/partner-shop-application/AGENTS.md` — `partner-shop-application` crate.
- `src/partner-shop-application-api/AGENTS.md` — `partner-shop-application-api` crate.
- `src/partner-shop-application-lambda/AGENTS.md` — `partner-shop-application-lambda` crate.
- `src/product/AGENTS.md` — `product` crate.
- `src/product-api/AGENTS.md` — `product-api` crate.
- `src/product-api-partner/AGENTS.md` — `product-api-partner` crate.
- `src/product-lambda/AGENTS.md` — `product-lambda` crate.
- `src/product-personalization/AGENTS.md` — `product-personalization` crate.
- `src/product-pipeline/AGENTS.md` — `product-pipeline` crate.
- `src/product-watchlist/AGENTS.md` — `product-watchlist` crate.
- `src/product-watchlist-api/AGENTS.md` — `product-watchlist-api` crate.
- `src/search-filter/AGENTS.md` — `search-filter` crate.
- `src/search-filter-api/AGENTS.md` — `search-filter-api` crate.
- `src/search-filter-lambda/AGENTS.md` — `search-filter-lambda` crate.
- `src/search-filter-periodic-match/AGENTS.md` — `search-filter-periodic-match` crate.
- `src/shop/AGENTS.md` — `shop` crate.
- `src/shop-api/AGENTS.md` — `shop-api` crate.
- `src/shop-lambda/AGENTS.md` — `shop-lambda` crate.
- `src/shopify-lambda/AGENTS.md` — `shopify-lambda` crate.
- `src/stripe-api/AGENTS.md` — `stripe-api` crate.
- `src/stripe-lambda/AGENTS.md` — `stripe-lambda` crate.
- `src/test-api/AGENTS.md` — `test-api` crate.
- `src/user/AGENTS.md` — `user` crate.
- `src/user-api/AGENTS.md` — `user-api` crate.
- `src/user-lambda/AGENTS.md` — `user-lambda` crate.
- `src/webhook-api/AGENTS.md` — `webhook-api` crate.
