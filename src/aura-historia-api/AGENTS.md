# DOX

## Purpose

- Own axum REST API runtime and transport auth service for #1341.

## Core Design

- `main.rs` bootstraps logging, config, and graceful shutdown.
- `lib.rs` owns runtime config, axum router, health/readiness endpoints, server loop, and composition root wiring.
- `state.rs` owns axum application state shared by route modules.
- `error.rs` owns API problem JSON errors.
- `auth/` owns bearer auth extraction, Cognito JWT verification via cached JWKS, Aura access-token auth, and mapping to `OperationContext`.
- Auth accepts Cognito access JWTs and Aura access tokens through one interface. Cognito needs `AURA_HISTORIA_COGNITO_ISSUER`, `AURA_HISTORIA_COGNITO_JWKS_URL`, and comma-separated `AURA_HISTORIA_COGNITO_APP_CLIENT_IDS`; it fetches JWKS with bounded cache/refresh. Cognito maps to open-world first-party `Principal::User`; Aura access tokens map explicit scopes to closed-world delegated capabilities.
- Auth extractors only authenticate. Required capability and business policy checks belong in service/use-case code, not controllers.
- Global axum transport middleware creates UUID request IDs; it accepts only bounded safe `X-Correlation-Id` values (max 128 ASCII alphanumeric, `.`, `_`, `-`) and returns both IDs on every response. It also owns safe request tracing, sensitive-header redaction, CORS, a 1 MiB body cap, and a 30-second timeout.
- `/health` reports process liveness. `/ready` returns `204` only after PostgreSQL pool acquisition, DynamoDB table lookup, and OpenSearch ping succeed; it returns `503` otherwise.
- No API Gateway adapter.
- `shops/` owns shop REST controllers. Public shop list and detail routes return only `PUBLISHED` shops; partner-application approval publishes its linked shop.
- `users/` owns account, admin user, and access-token REST controllers.
- `watchlist/` owns watchlist REST controllers. Product watchlist paths now use `{productId}` only. `GET /api/v1/me/watchlist` uses Postgres-backed common `Cursor`/`CursoredResult` pagination and common JSON cursor collection data; its tie-safe `searchAfter` is `[created RFC3339 timestamp, product UUID]`. It returns `PersonalizedData<ProductDetailsData, ProductUserStateData>` entries with `no-store`. Watch creation and inactive-to-active PATCH enforce active-entry quotas: Free 20, Pro 100, Ultimate unlimited.
- `search_filters/` owns Postgres-backed saved-search filter CRUD and match-feedback REST controllers.
- `products/` owns canonical product detail, search, immutable history, and similar-product REST controllers. Detail, history, and similar routes use product ID or shop/product slugs. Canonical detail, search, KNN, and watchlist Product values always serialize as `PersonalizedData` with required `item` and optional `userState`. Detail and watchlist use joined Postgres reads; search and KNN use denormalized OpenSearch fields, then service-owned batched Postgres plus DynamoDB hydration for valid user/delegated-user tokens. Image values always expose `prohibitedContent`; unsafe image URLs are omitted without effective consent. Product history uses the no-consent redaction default. Product prices remain stored source amounts/currencies with `fxRateId`; currency conversion is deferred to #1466.
- `partner_applications/` owns own/admin partner-shop application REST controllers.
- `partner_products/` owns synchronous partner Product batch create, update, upsert, and delete controllers at `POST`, `PATCH`, `PUT`, and `DELETE /api/v1/shops/{shopId}/products`. Batches allow at most 100 entries; each entry calls one service use case; partial batches return `200` with failed `{ shopId, shopsProductId }` items.
- `oauth/` owns OAuth REST controllers for client registration, authorization code, token, revoke, and introspection flows.
- Runtime shop and partner-shop structured-address writes use one shared Google geocoder adapter. `GOOGLE_GEOCODING_API_KEY` is required at startup.
- Product text search uses native OpenSearch hybrid BM25 plus KNN when relevance-sorted query embedding succeeds; embedding failure and explicit non-score sorts use BM25. Product search runtime needs `OPENSEARCH_ENDPOINT_URL`; outside `STAGE=ephemeral`, it also needs `OPENSEARCH_USERNAME` and `OPENSEARCH_PASSWORD`.
- Search-filter create/update embeddings use Vertex AI Gemini through typed API config. `VERTEX_AI_PROJECT_ID` and `VERTEX_AI_LOCATION` may override the legacy project and `eu` defaults; Google ADC supplies credentials (normally `GOOGLE_APPLICATION_CREDENTIALS`).

## Ownership

- This doc rule `src/aura-historia-api/**`.
- Parent doc: `src/AGENTS.md`.

## Local Contracts

- Read repo root, `src/AGENTS.md`, then here before edit.
- Update this doc when env vars, route shape, dependencies, or runtime behavior changes.
- Public API route behavior must update `docs/swagger.yaml` and `docs/CHANGELOG.md` when routes become real.

## Work Guidance

- Keep runtime glue thin.
- Put business behavior in domain crates and services.
- Use runtime-neutral request/auth context; no API Gateway context.

## Verification

- `cargo check -p aura-historia-api`
- `cargo test -p aura-historia-api --all-features`

## Child DOX Index

- `shops/` — shop REST controllers.
- `users/` — user account, admin, and access-token REST controllers.
- `watchlist/` — watchlist REST controllers.
- `partner_applications/` — own/admin partner-shop application REST controllers.
- `partner_products/` — synchronous partner Product batch write controllers.
- `oauth/` — OAuth REST controllers.
- `products/` — canonical product detail and search REST controllers.
- `search_filters/` — saved-search filter and match REST controllers.
