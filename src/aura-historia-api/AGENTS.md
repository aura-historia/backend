# DOX

## Purpose

- Own axum REST API runtime and transport auth service for #1341.

## Core Design

- `main.rs` bootstraps logging, config, and graceful shutdown.
- `lib.rs` owns runtime config, axum router, health/readiness endpoints, server loop, and composition root wiring.
- `state.rs` owns axum application state shared by route modules.
- `error.rs` owns API problem JSON errors.
- `wire.rs` owns REST codecs for canonical semantic leaf types. DTO structs keep JSON shape and use codecs; API does not duplicate canonical enums only for Serde.
- `auth/` owns bearer auth extraction, Cognito JWT verification via cached JWKS, Aura access-token auth, and mapping to `OperationContext`.
- Auth accepts Cognito access JWTs and Aura access tokens through one interface. Cognito needs `AURA_HISTORIA_COGNITO_ISSUER`, `AURA_HISTORIA_COGNITO_JWKS_URL`, and comma-separated `AURA_HISTORIA_COGNITO_APP_CLIENT_IDS`; it fetches JWKS with bounded cache/refresh. Cognito maps to open-world first-party `Principal::User`; Aura access tokens map explicit scopes to closed-world delegated capabilities.
- Auth extractors only authenticate. Required capability and business policy checks belong in service/use-case code, not controllers.
- Global axum transport middleware creates UUID request IDs; it accepts only bounded safe `X-Correlation-Id` values (max 128 ASCII alphanumeric, `.`, `_`, `-`) and returns both IDs on every response. It also owns safe request tracing, sensitive-header redaction, CORS including WooCommerce topic/signature headers, a 1 MiB body cap, and a 30-second timeout.
- `/health` reports process liveness. `/ready` returns `204` only after PostgreSQL pool acquisition and OpenSearch ping succeed; it returns `503` otherwise. pg-ttl worker health is monitored as PostgreSQL platform health, not a request-path readiness dependency.
- No API Gateway adapter.
- `shops/` owns shop REST controllers. Public shop list and detail routes return only `PUBLISHED` shops; partner-application approval publishes its linked shop.
- `users/` owns account, admin user, and access-token REST controllers.
- `newsletter/` owns public newsletter subscription REST controller. It uses optional canonical auth and a User service use case; production wiring reads Postgres user-profile fallback data and writes Zoho Campaigns subscriptions.
- `notifications/` owns authenticated canonical notification list, seen-state update, and deletion REST controllers. It uses PostgreSQL-backed notification service use cases; list items expose a localized immutable reason-specific rendering snapshot but never origin-event or delivery/provider state. Watchlist price changes preserve event source currency and ignore user currency preferences. Product image classifications remain visible, while unsafe image URLs are omitted without the owner’s current prohibited-content consent. Notification item paths use canonical `{notificationId}` values and list pagination uses an opaque JSON `[created RFC3339 timestamp, notification UUID]` cursor.
- `watchlist/` owns watchlist REST controllers. ProductListing watchlist paths use `{productListingId}`. `GET /api/v1/me/watchlist` uses Postgres-backed `application` `Cursor`/`CursoredResult` pagination and API-local JSON cursor collection data; its tie-safe `searchAfter` is `[created RFC3339 timestamp, ProductListing UUID]`. It returns personalized ProductListing details with `no-store`. Watch creation and inactive-to-active PATCH enforce active-entry quotas: Free 20, Pro 100, Ultimate unlimited.
- `search_filters/` owns Postgres-backed saved-search filter CRUD and match-feedback REST controllers. Raw enhanced descriptions are validated at transport mapping before embedding or any write transaction.
- `billing/` owns authenticated Stripe checkout, portal, and management REST controllers backed by canonical User state and Stripe billing service use cases. Runtime requires `STRIPE_API_KEY`, checkout success/cancel URLs, portal return URL, and configured Pro/Ultimate monthly/yearly price IDs.
- `product_listings/` owns ProductListing detail, search, immutable history, and similar-listing controllers. Canonical routes use `/api/v1/product-listings`, `productListingId`, `productListingSlugId`, and `shopListingId`. Listing availability uses the canonical `availability` field and `ListingAvailability` codec; lifecycle uses `ListingLifecycle`. Canonical detail, search, KNN, and watchlist ProductListing values serialize as `PersonalizedData` with required `item` and optional `userState`. Detail and watchlist use joined Postgres reads; search and KNN use denormalized OpenSearch fields, then service-owned batched Postgres hydration for valid user/delegated-user tokens. Image values always expose `prohibitedContent`; unsafe image URLs are omitted without effective consent. ProductListing history uses the no-consent redaction default. ProductListing detail, watchlist, saved-search match, and similar pricing accept a `currency` query (default `EUR`). Full detail returns source and converted display pricing; search/KNN summaries return `displayPrice` plus explicit current-or-sale valuation metadata. Existing sale valuation behavior is unchanged. Anonymous ProductListing detail cache responses use freshness directives only: no `ETag` or `Last-Modified` validator is emitted because current FX selection can change display pricing.
- `partner_applications/` owns own/admin partner-shop application REST controllers.
- `partner_product_listings/` owns synchronous partner ProductListing batch create, update, upsert, and DELETE-route withdrawal controllers at `/api/v1/shops/{shopId}/product-listings`. Batches use `shopListingId`, allow at most 100 entries, and call one ProductListing service use case per entry.
- `webhooks/` owns direct WooCommerce product webhook transport. It receives the raw provider body plus topic/signature headers and delegates one ProductListing intake use case, which validates the signed partner request and persists canonical listing state; successful requests return `204` only after the authoritative Postgres write commits.
- `oauth/` owns OAuth REST controllers for client registration, authorization code, token, revoke, and introspection flows. Service use cases enforce delegated `access-tokens:read`/`write` capabilities; authorization requests cannot exceed delegated caller scopes.
- Runtime shop and partner-shop structured-address writes use one shared Google geocoder adapter. `GOOGLE_GEOCODING_API_KEY` is required at startup.
- Newsletter runtime requires `ZOHO_LIST_KEY`, `ZOHO_CLIENT_ID`, `ZOHO_CLIENT_SECRET`, `ZOHO_REFRESH_TOKEN`, `ZOHO_ACCOUNTS_URL`, and `ZOHO_CAMPAIGNS_URL`; credentials and contact payloads are never logged.
- ProductListing text search first pages use a short repository read transaction for the latest persisted FX snapshot; continuation pages load the ProductListing cursor's pinned snapshot ID. It then uses native OpenSearch hybrid BM25 plus KNN when relevance-sorted query embedding succeeds; embedding failure and explicit non-score sorts use BM25. ProductListing search runtime needs `OPENSEARCH_ENDPOINT_URL`; outside `STAGE=ephemeral`, it also needs `OPENSEARCH_USERNAME` and `OPENSEARCH_PASSWORD`. Black-box ProductListing search, partner-write, and WooCommerce webhook tests seed a complete persisted FX snapshot after each PostgreSQL fixture reset so current-FX reads and `SOLD` transitions exercise their real invariants.
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
- API implements no service port or use case. It only maps HTTP/auth transport and composes adapters from adapter crates. Transport-local auth and readiness traits are allowed.
- Put business behavior in domain crates and services.
- Use runtime-neutral request/auth context; no API Gateway context.

## Verification

- `cargo check -p aura-historia-api`
- `cargo test -p aura-historia-api --all-features`

## Child DOX Index

- `shops/` — shop REST controllers.
- `users/` — user account, admin, and access-token REST controllers.
- `newsletter/` — public newsletter subscription REST controller.
- `notifications/` — canonical notification REST controllers.
- `watchlist/` — watchlist REST controllers.
- `partner_applications/` — own/admin partner-shop application REST controllers.
- `partner_product_listings/` — synchronous partner ProductListing batch write controllers.
- `oauth/` — OAuth REST controllers.
- `product_listings/` — canonical ProductListing detail and search REST controllers.
- `search_filters/` — saved-search filter and match REST controllers.
- `billing/` — Stripe billing session REST controllers.
- `webhooks/` — WooCommerce product webhook REST controller.
