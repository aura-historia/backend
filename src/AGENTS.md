# DOX

## Purpose

- Own Rust workspace map.
- Own `src/lib.rs` and `src/opensearch/`.
- Hold global Rust work rules for all crates under `src/`.

## Core Design

- Workspace split by job: domain libs hold rules, `*-api`/`aura-historia-api` crates speak HTTP, `aura-historia-worker` handles async CDC/queues, survivor `*-lambda` crates speak AWS event/runtime, test crates prove behavior.
- Keep reusable logic in domain or service modules. Handler `main.rs`, route files, and Lambda bootstrap stay thin.
- Shared OpenSearch assets under `src/opensearch/` stay governed here unless they grow own durable boundary.
- Crate submodule-design
  - core: domain logic and business rules
  - data: REST-API payloads
  - dynamodb: DynamoDB payloads
  - opensearch: OpenSearch payloads
  - service: service glue, orchestration, and cross-crate integration
- DynamoDB is current source-of-truth for not-yet-migrated records. Postgres is target business truth per `docs/storage.md`. OpenSearch is re-computable read-optimized view for search and discovery.
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
- If OpenSearch DTOs change, make sure the corresponding index-mappings in `opensearch/mappings` are aligned
- If relevant event structure or flow change, update `docs/events/flow.md`
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
- Apply semi-strict DDD. Keep domain logic in domain modules, services thin, and use newtypes.
- Keep type layers strict: domain no suffix, REST `Data`, DynamoDB `Record`, OpenSearch `Document`.

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
- Use `rstest` for table tests and `fake` where applicable.
- Use `..Default::default()` in tests to avoid boilerplate.
- Unit-test public functions and critical private functions.
- Integration-test repository functions and Lambda happy paths.
- Acceptance-test critical REST, Lambda, event, business-rule, and auth/user-plan flows.

## Runtime Guidance

- Canonical composition roots init logging with `platform-observability`; legacy executables may use `common::logging` shim.
- Keep logs compact & structured for CloudWatch-Analysis. Error log mean real fire. Expected failure be `warn` or lower.
- Do not hide business rules in handler glue. Parse, auth, and map in edge crate; real rule live deeper.
- Keep env var names, queue names, and event shapes stable and documented in nearest crate doc.

## Verification

- Whole workspace: `cargo check --workspace`
- Whole unit tests: `cargo test --workspace --lib --all-features`

## Child DOX Index

- `src/acceptance-tests/AGENTS.md` — `acceptance-tests` crate.
- `src/application/AGENTS.md` — shared technology-neutral application contracts.
- `src/aura-historia-api/AGENTS.md` — `aura-historia-api` crate.
- `src/aura-historia-worker/AGENTS.md` — `aura-historia-worker` crate.
- `src/aws-tests/AGENTS.md` — `aws-tests` crate.
- `src/ci-determinator/AGENTS.md` — `ci-determinator` crate.
- `src/cloudwatch-log-retention-lambda/AGENTS.md` — `cloudwatch-log-retention-lambda` crate.
- `src/cognito/AGENTS.md` — `cognito` crate.
- `src/cognito-post-confirmation/AGENTS.md` — `cognito-post-confirmation` crate.
- `src/common/AGENTS.md` — legacy compatibility crate.
- `src/credential-core/AGENTS.md` — credential identifiers and scope vocabulary.
- `src/domain-primitives/AGENTS.md` — domain-neutral primitives and newtype macros.
- `src/embedding/AGENTS.md` — reusable Vertex AI embedding adapter crate.
- `src/image-fetcher/AGENTS.md` — reusable safe external image-fetch adapter crate.
- `src/large-language-model/AGENTS.md` — reusable typed Vertex AI Gemini invocation adapter crate.
- `src/localization/AGENTS.md` — pure language and localization values.
- `src/money/AGENTS.md` — pure currency, amount, and price values.
- `src/crawler/AGENTS.md` — `crawler` crate.
- `src/fxrate-core/AGENTS.md` — canonical FX domain crate.
- `src/fxrate-service/AGENTS.md` — canonical FX service/use-case crate.
- `src/fxrate-postgres/AGENTS.md` — canonical FX PostgreSQL adapter crate.
- `src/fxrate-fxratesapi/AGENTS.md` — canonical FxRatesApi adapter crate.
- `src/fxrate-lambda/AGENTS.md` — scheduled FX capture Lambda.
- `src/geo/AGENTS.md` — `geo` crate.
- `src/newsletter-api/AGENTS.md` — `newsletter-api` crate.
- `src/notification/AGENTS.md` — legacy notification crate.
- `src/notification-core/AGENTS.md` — canonical Notification domain crate.
- `src/notification-service/AGENTS.md` — canonical Notification service/use-case crate.
- `src/notification-dynamodb/AGENTS.md` — canonical Notification DynamoDB adapter crate.
- `src/notification-api/AGENTS.md` — `notification-api` crate.
- `src/notification-send/AGENTS.md` — `notification-send` crate.
- `src/oauth/AGENTS.md` — legacy `oauth` crate.
- `src/oauth-api/AGENTS.md` — legacy `oauth-api` crate.
- `src/oauth-core/AGENTS.md` — canonical OAuth domain crate.
- `src/oauth-service/AGENTS.md` — canonical OAuth service/use-case crate.
- `src/oauth-dynamodb/AGENTS.md` — canonical OAuth DynamoDB adapter crate.
- `src/partner-shop-application/AGENTS.md` — `partner-shop-application` crate.
- `src/partner-shop-application-api/AGENTS.md` — `partner-shop-application-api` crate.
- `src/partner-shop-application-lambda/AGENTS.md` — `partner-shop-application-lambda` crate.
- `src/product/AGENTS.md` — legacy `product` crate.
- `src/product-core/AGENTS.md` — canonical Product domain crate.
- `src/product-service/AGENTS.md` — canonical Product service crate.
- `src/product-postgres/AGENTS.md` — canonical Product Postgres adapter crate.
- `src/platform-observability/AGENTS.md` — typed tracing subscriber setup.
- `src/platform-postgres/AGENTS.md` — shared SQLx transaction and pool mechanics.
- `src/product-opensearch/AGENTS.md` — canonical Product OpenSearch adapter crate.
- `src/product-api/AGENTS.md` — `product-api` crate.
- `src/product-api-partner/AGENTS.md` — `product-api-partner` crate.
- `src/product-lambda/AGENTS.md` — `product-lambda` crate.
- `src/product-personalization/AGENTS.md` — `product-personalization` crate.
- `src/product-watchlist/AGENTS.md` — legacy `product-watchlist` crate.
- `src/watchlist-core/AGENTS.md` — canonical Watchlist domain crate.
- `src/watchlist-service/AGENTS.md` — canonical Watchlist service crate.
- `src/watchlist-postgres/AGENTS.md` — canonical Watchlist Postgres adapter crate.
- `src/product-watchlist-api/AGENTS.md` — `product-watchlist-api` crate.
- `src/search-filter/AGENTS.md` — legacy `search-filter` crate.
- `src/search-filter-core/AGENTS.md` — canonical Search Filter domain crate.
- `src/search-filter-service/AGENTS.md` — canonical Search Filter service crate.
- `src/search-filter-postgres/AGENTS.md` — canonical Search Filter Postgres adapter crate.
- `src/search-filter-opensearch/AGENTS.md` — canonical Search Filter OpenSearch adapter crate.
- `src/search-filter-api/AGENTS.md` — `search-filter-api` crate.
- `src/search-filter-lambda/AGENTS.md` — `search-filter-lambda` crate.
- `src/search-filter-periodic-match/AGENTS.md` — `search-filter-periodic-match` crate.
- `src/shop/AGENTS.md` — legacy `shop` crate.
- `src/shop-core/AGENTS.md` — canonical Shop domain crate.
- `src/shop-service/AGENTS.md` — canonical Shop service crate.
- `src/shop-postgres/AGENTS.md` — canonical Shop Postgres adapter crate.
- `src/shop-partner-core/AGENTS.md` — canonical Partner Shop Application domain crate.
- `src/shop-partner-service/AGENTS.md` — canonical Partner Shop Application service crate.
- `src/shop-partner-postgres/AGENTS.md` — canonical Partner Shop Application Postgres adapter crate.
- `src/shop-api/AGENTS.md` — `shop-api` crate.
- `src/shop-lambda/AGENTS.md` — `shop-lambda` crate.
- `src/shopify-lambda/AGENTS.md` — `shopify-lambda` crate.
- `src/stripe-api/AGENTS.md` — `stripe-api` crate.
- `src/stripe-lambda/AGENTS.md` — `stripe-lambda` crate.
- `src/test-api/AGENTS.md` — `test-api` crate.
- `src/user/AGENTS.md` — legacy `user` crate.
- `src/user-core/AGENTS.md` — canonical User domain crate.
- `src/user-service/AGENTS.md` — canonical User service crate.
- `src/user-dynamodb/AGENTS.md` — canonical User DynamoDB adapter crate.
- `src/user-postgres/AGENTS.md` — canonical User Postgres adapter crate.
- `src/user-zoho/AGENTS.md` — canonical User Zoho newsletter adapter crate.
- `src/user-api/AGENTS.md` — `user-api` crate.
- `src/user-lambda/AGENTS.md` — `user-lambda` crate.
- `src/webhook-api/AGENTS.md` — `webhook-api` crate.
