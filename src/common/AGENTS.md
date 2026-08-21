# DOX

## Purpose

- Own legacy `common` compatibility crate.

## Core Design

- Legacy compatibility for existing consumers while canonical code moves to narrow semantic owners.
- `personalized::Personalized<Item, UserState>`, `patch_field`, `pagination` core values, and boxed errors are legacy shims to `application`; legacy API DTO forms remain local here until legacy consumers migrate.
- As lean as possible.
- Root modules: `actor`, `currency`, `distance`, `api`, `batch`, `change_outcome`, `domain`, `enhanced_match_reason`, `dynamodb_update`, `dynamodb_stream`, `error`, `event`, `event_id`, `fx_rate_id`, `execution_state`, `fake`, `has_key`, `language`, `localized`, `logging`, `measurement_unit`, `mergeable`, `notification_id`, `product_id`, `product_lifecycle`, `product_slug_id`, `product_state`, `oauth_client_id`, `operation_context`, `opensearch`, `pagination`, `patch_field`, `postgres`, `partner_shop_application_id`, `personalized`, `price`, `query`, `resource_state`, `seller_slug_id`, `shop_id`, `shop_name`, `shop_slug_id`, `shops_product_id`, `slug_id`, `sort`, `string_newtype`, `stripe_customer_id`, `transaction`, `user_id`, `user_search_filter_id`, `user_search_filter_name`, `utm`, `uuid_newtype`, `version`, `versioned`, `year`.
- Library crate. Keep domain, persistence, and service seams explicit.
- `operation_context` re-exports application-owned principals and context. Cognito user sessions are `Principal::User`; Aura access tokens are `Principal::DelegatedUser` with explicit closed-world capabilities.
- `Principal` exposes lean `require*` guards and chainable principal requirements. Use `Principal` in services instead of direct `Actor` matching.
- `change_outcome`, `event`, and `event_id` alias `domain-primitives`; legacy EventId API extraction remains local. Version and newtype macro modules remain legacy copies while canonical owners use `domain-primitives`. Remove each legacy path after consumers migrate.
- `logging` delegates subscriber setup to `platform-observability` and retains legacy log vocabulary. Remove the setup shims after legacy runtimes migrate.
- `transaction`, `operation_context`, boxed errors, pagination core values, patch fields, and personalization re-export `application`; `postgres` re-exports SQLx primitives from `platform-postgres` and retains legacy `POSTGRES_*` parsing. `opensearch::search_response` re-exports the generic envelope from `platform-opensearch`. Remove shims after legacy consumers migrate.
- `user_id` and `stripe_customer_id` re-export `user-core`; `oauth_client_id` re-exports `credential-core`; search-filter IDs, name, and enhanced match reason re-export `search-filter-core`; partner application ID re-exports `shop-partner-core`. `domain`, shop IDs, distance, FX, Product IDs, and query values re-export their canonical owners. Remove these shims after legacy consumers migrate. Legacy boundary DTOs remain only to preserve boundary forms.
- Legacy `product_state` and `product_lifecycle` remain separate until their legacy boundary mappings migrate; canonical code must use `product-core` values.
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
