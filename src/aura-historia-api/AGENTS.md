# DOX

## Purpose

- Own axum REST API runtime and transport auth service for #1341.

## Core Design

- `main.rs` bootstraps logging, config, and graceful shutdown.
- `lib.rs` owns runtime config, axum router, health/readiness endpoints, server loop, and composition root wiring.
- `state.rs` owns axum application state shared by route modules.
- `error.rs` owns API problem JSON errors.
- `auth/` owns bearer auth extraction, Cognito JWT verification via cached JWKS, Aura access-token auth, and mapping to `OperationContext`.
- Auth accepts Cognito JWTs and Aura access tokens through one interface. Cognito maps to open-world first-party `Principal::User`; Aura access tokens map explicit scopes to closed-world delegated capabilities.
- Auth extractors only authenticate. Required capability and business policy checks belong in service/use-case code, not controllers.
- Request IDs are server-created by future axum middleware; clients may only provide correlation IDs if middleware accepts them.
- No API Gateway adapter.
- `shops/` owns shop REST controllers.
- `users/` owns account, admin user, and access-token REST controllers.
- `watchlist/` owns watchlist REST controllers. Product watchlist paths now use `{productId}` only.
- `products/` owns canonical product detail, search, immutable history, and similar-product REST controllers. Detail, history, and similar routes use product ID or shop/product slugs. Product detail uses one joined Postgres reader query and adds top-level `userState` for valid user/delegated-user tokens; its handler then reads the newest DynamoDB notification after Postgres commits. Search and KNN similar reads use denormalized OpenSearch fields and are not personalized yet. Product prices remain stored source amounts/currencies with `fxRateId`; currency conversion is deferred to #1466.
- `partner_applications/` owns own/admin partner-shop application REST controllers.
- `oauth/` owns OAuth REST controllers for client registration, authorization code, token, revoke, and introspection flows.
- Runtime shop and partner-shop create/update geocoding is not wired yet; structured-address writes return temporary failure until a geocoder adapter is added.
- Product search runtime needs `OPENSEARCH_ENDPOINT_URL`; outside `STAGE=ephemeral`, it also needs `OPENSEARCH_USERNAME` and `OPENSEARCH_PASSWORD`.

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
- `partner_applications/` — partner-shop application REST controllers.
- `oauth/` — OAuth REST controllers.
- `products/` — canonical product detail and search REST controllers.
