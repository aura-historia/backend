---
name: aura-rust-api-endpoint
description: Use when adding or changing Aura Historia Rust API endpoints, axum controllers, REST DTOs, authentication extraction, OperationContext mapping, ApiError mappings, or route tests.
---

# Aura Rust API Endpoint

Use for `aura-historia-api` route/controller work.

## Must read

- `backend/AGENTS.md` and path `AGENTS.md` files.
- `docs/arch.md` §3.5-3.6, §10.1, §13, §15, §18-19, §20.7-20.8, §21-23.

## Before coding

- Find route unit module and endpoint file.
- Find inbound use-case trait expected in state.
- Decide protected vs public optional-auth behavior.
- Identify request/response DTOs and error mappings.
- Check if public docs or Swagger need update.

## Controller owns

- REST path/query/header extraction.
- Authentication principal extraction.
- Mapping transport identity to service-owned `OperationContext`.
- Request/response DTOs.
- Transport-level validation.
- Request-to-command/request mapping.
- Inbound use-case invocation.
- Result/view-to-response mapping.
- Cache/representation headers from REST contract.
- Service-error-to-HTTP mapping through crate error mappings.

## Hard rules

- One file per endpoint. Group endpoints in sensible modules for the specific entity.
- Route files stay thin: authenticate, map, call use case, map result/error.
- `aura-historia-api/state.rs` owns axum state structs.
- State should hold inbound use-case trait objects and authenticator trait objects, not repositories.
- Controllers depend on inbound use-case traits, not handlers or adapters.
- REST DTOs belong to API module and stay private or `pub(crate)`.
- Use `TryFrom` for fallible request mapping and `From` for infallible response mapping.
- Service MUST NOT know REST DTOs or HTTP status codes.
- Use crate-local `ApiError` in `error.rs` for HTTP failures.
- Reusable service-error mappings belong as `From<ErrorType> for ApiError` in `error.rs` or re-exported error mapping module.
- Public problem JSON error codes are stable API and should use `ApiErrorCode` constants, not inline strings.
- Missing auth on public optional-auth route maps to anonymous.
- Invalid supplied auth must be rejected, not downgraded to anonymous.
- Protected endpoint route auth is not enough; use case enforces business authorization.
- Command endpoint should not perform follow-up read just to assemble response.

## Controller MUST NOT

- Access database clients.
- Call repositories/readers directly.
- Build SQL or search DSL.
- Compose multiple data sources.
- Construct concrete adapters.
- Enforce business authorization policy.
- Enforce domain invariants.
- Mutate aggregates.
- Return storage types.
- Decide transaction scope.

## Tests

- Test routers/controllers with fake inbound use-case traits and fake authenticators.
- Cover success, DTO mapping, status/error mapping, response DTO shape, auth rejection, optional-auth missing-token behavior, contract headers, and protected-route auth enforcement.
- Acceptance tests call the public REST API only and use `test-api::AuraHistoriaApi` when relevant.
