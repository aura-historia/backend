---
name: add-rest-api-endpoint
description: Use when adding or changing a REST API endpoint in the Aura Historia backend. Covers route selection, Rust API handler shape, auth, payload/error contracts, infra route wiring, Swagger/changelog updates, tests, and validation.
---

# Add REST API Endpoint

Use this skill when a task adds or changes an HTTP route exposed by API Gateway.
If the endpoint needs a brand-new Lambda binary first, use `add-backend-lambda` for that wiring, then return here.

## First checks

- Read the required `AGENTS.md` chain: repo root, `src/AGENTS.md`, target `*-api/AGENTS.md`, `infra/AGENTS.md`, and `docs/AGENTS.md` before editing those areas.
- Inspect sibling endpoints in the same API crate before designing. Good samples:
  - dispatch: `src/user-api/src/lib.rs`, `src/product-api/src/lib.rs`
  - route modules: `src/user-api/src/patch.rs`, `src/product-api/src/get_product.rs`
  - infra routes: `infra/src/constructs/api.ts`
  - public contract: `docs/swagger.yaml`, `docs/CHANGELOG.md`
- Define the public contract first: method, path, auth, path/query params, body, response body, status codes, cache headers, and error behavior.

## Choose route home

- Prefer an existing `src/<domain>-api/` crate when the endpoint belongs to that domain.
- Create a new API Lambda only when isolation, ownership, dependencies, or deployment shape justify it. If so, use `add-backend-lambda`.
- Keep `*-api` crates as HTTP edge only: parse, auth, call service, map response.
- Put business rules in domain/service crates, not route files.

## Rust route shape

- Add a focused route module under `src/<domain>-api/src/`, e.g. `get.rs`, `post.rs`, `patch.rs`, or a noun module when several verbs share code.
- Export/register it in `src/<domain>-api/src/lib.rs`:
  - add `mod`/`pub mod` following local style
  - add an exact `route_key` match like `Some("PATCH /api/v1/me/account")`
  - unknown or missing route keys should map to `ApiError::internal_server_error(INTERNAL_SERVER_ERROR, ...)`
- Keep `handler` wrapping `handle` and converting `ApiError` via `log_api_error` + `ApiGatewayV2httpResponse::from(err)`.
- Use `LambdaEvent<ApiGatewayV2httpRequest>` and return `Result<ApiGatewayV2httpResponse, ApiError>` from route handlers.

## Request parsing

- Use existing extractor helpers from `common::*::api` for path and query parameters.
- Add reusable extractors to `common` or the domain crate when the same parsing will be reused.
- For required JSON bodies:
  - reject missing/empty body with `ApiError::bad_request(BAD_BODY_VALUE, ...)`
  - map `serde_json::from_str` errors to `BAD_BODY_VALUE` with useful details
- For query strings with arrays or complex filters, match the crate's existing parser (`serde_qs` where already used).
- Do not use `.ok()` to drop errors. Propagate typed errors with `?`.

## Auth rules

- For required Cognito JWT routes:
  - set `authenticated: true` in `infra/src/constructs/api.ts`
  - extract user from API Gateway request context, e.g. `extract_user_id_request_context(...)`
  - record `userId` on the tracing span
- For optional personalization/auth:
  - keep the route public in infra
  - use `AccessTokenVerifierService::verify_extract_user_id(&event.payload.headers)`
  - record `userId` only when present
- For admin/user-targeted routes, check authorization in service/domain code, not only in the route.

## Response and cache rules

- Build responses with `ApiGatewayV2HttpResponseBuilder`.
- Use REST DTOs with `Data` suffix only, e.g. `UserData`; domain types have no suffix.
- Set status codes intentionally:
  - `200` with body for reads/updates returning data
  - `201` for created resources when the API contract expects it
  - `204` for success with no body
- Add useful headers when available: `Last-Modified`, `ETag`, `Content-Language`.
- Cache public `GET` responses only when safe.
- Use `no-store` for authenticated, personalized, or user-specific responses.
- Never expose secrets, tokens, or internal error details in response bodies.

## Error mapping

- Prefer well-typed `thiserror` enums in service/domain crates.
- Map domain/service errors to `ApiError` near the edge or through existing `From` impls.
- Cover expected user failures with 4xx status codes.
- Reserve 5xx for real server failures.
- Keep `error` logs for real fires; expected validation/auth failures should be lower severity through existing API error logging.

## Infra route wiring

Update `infra/src/constructs/api.ts` in the same change:

- Add a `route("METHOD", "/api/v1/...", "lambdaKey", authenticated?)` entry.
- Use the same method/path string as the Rust `route_key` match and tests.
- If adding a new header clients may send, update CORS `allowHeaders`.
- If path params are present, ephemeral LocalStack broad invoke grants are handled by existing code.
- If the route targets a new Lambda, complete `add-backend-lambda` infra wiring first.

## Public docs

When endpoint, payload, auth, or error behavior changes, update:

- `docs/swagger.yaml`
  - path/method, security, params, request body, responses, schemas, examples
- `docs/CHANGELOG.md`
  - short user-facing contract change
- `docs/dynamodb/table_1.md` if DynamoDB shape changes
- `docs/events/flow.md` if the endpoint emits or changes events
- OpenSearch mappings under `opensearch/mappings` if DTO/index shape changes

## Tests

- Unit-test route modules with `test_api::ApiGatewayV2httpRequestProxy` and mocks.
- Cover:
  - happy path
  - missing/invalid auth
  - missing/invalid path/query/body
  - service/domain error mapping
  - response headers and cache behavior
  - optional auth/personalization branches when present
- Add route-dispatch tests when the crate already tests dispatch or the match is non-trivial.
- Add acceptance tests for critical public flows or cross-crate behavior. Every endpoint should be invoked in at least one acceptance test.
- Use `rstest` for tables and `fake`/`test-data` features when existing crate patterns support it.

## DOX updates

- Update target `*-api/AGENTS.md` when route list, auth, payload shape, env vars, dependencies, or verification changes.
- Update domain crate docs if service/domain contract changes.
- Update `src/AGENTS.md` child index only for new top-level crates.

## Validation

Start narrow, then grow:

1. `cargo check -p <api-crate>`
2. `cargo test -p <api-crate> --all-features`
3. If domain/common crates changed: run their package tests too
4. If infra route changed: `npm --prefix infra test` and relevant synth
5. If public docs changed, inspect `docs/swagger.yaml` for schema/path consistency
6. For broad changes: `cargo check --workspace`

Report skipped acceptance or LocalStack tests clearly.
