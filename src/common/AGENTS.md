# DOX

## Purpose

- Own legacy `common` compatibility crate.

## Core Design

- Legacy compatibility for existing consumers while canonical code moves to narrow semantic owners.
- No new production consumer, feature, or top-level module. `scripts/common-decomposition/baseline.json` is the shrinking allowlist.
- `personalized::Personalized<Item, UserState>` is the shared application wrapper; its API feature exposes matching `PersonalizedData` with required `item` and optional `userState`.
- As lean as possible.
- Root modules: `actor`, `currency`, `distance`, `api`, `batch`, `change_outcome`, `domain`, `enhanced_match_reason`, `dynamodb_update`, `dynamodb_stream`, `error`, `event`, `event_id`, `fx_rate_id`, `execution_state`, `fake`, `has_key`, `language`, `localized`, `logging`, `measurement_unit`, `mergeable`, `product_id`, `product_lifecycle`, `product_slug_id`, `product_state`, `oauth_client_id`, `operation_context`, `opensearch`, `pagination`, `patch_field`, `postgres`, `partner_shop_application_id`, `personalized`, `price`, `query`, `resource_state`, `seller_slug_id`, `shop_id`, `shop_name`, `shop_slug_id`, `shops_product_id`, `slug_id`, `sort`, `string_newtype`, `stripe_customer_id`, `transaction`, `user_id`, `user_search_filter_id`, `user_search_filter_name`, `utm`, `uuid_newtype`, `version`, `versioned`, `year`.
- Library crate. Keep domain, persistence, and service seams explicit.
- `operation_context` owns service principals and `CredentialCapability`. Cognito user sessions are `Principal::User` with open-world capability. Aura access tokens are `Principal::DelegatedUser` with explicit closed-world capabilities.
- `Principal` exposes lean `require*` guards and chainable principal requirements. Use `Principal` in services instead of direct `Actor` matching.
- `transaction` re-exports `application`; `postgres` re-exports SQLx primitives from `platform-postgres` and retains legacy `POSTGRES_*` parsing. Remove both shims after legacy consumers migrate.
- `price` FX helpers use scaled unsigned `Rate` values; do not calculate exchange rates with floating point.

## Ownership

- This doc rule `src/common/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, route/event shape, env vars, or child index change.
- Do not add business rules here. Put new behavior in its semantic owner.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Service and repository split stay clean.
- Keep transport and runtime glue out of domain core.

## Verification

- `cargo check -p common`
- `cargo test -p common --all-features`

## Child DOX Index

- None.
