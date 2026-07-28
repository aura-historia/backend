# DOX

## Purpose

- Own `common` crate.

## Core Design

- Shared primitives, IDs, logging, boxed error-source helpers, AWS helpers, and cross-crate utilities.
- As lean as possible.
- Root modules: `actor`, `currency`, `distance`, `api`, `batch`, `change_outcome`, `domain`, `enhanced_match_reason`, `dynamodb_update`, `dynamodb_stream`, `error`, `event`, `event_id`, `execution_state`, `fake`, `has_key`, `language`, `localized`, `logging`, `measurement_unit`, `mergeable`, `product_id`, `product_lifecycle`, `product_slug_id`, `product_state`, `oauth_client_id`, `operation_context`, `opensearch`, `pagination`, `patch_field`, `postgres`, `partner_shop_application_id`, `personalized`, `price`, `query`, `resource_state`, `seller_slug_id`, `shop_id`, `shop_name`, `shop_slug_id`, `shops_product_id`, `slug_id`, `sort`, `string_newtype`, `stripe_customer_id`, `transaction`, `user_id`, `user_search_filter_id`, `user_search_filter_name`, `utm`, `uuid_newtype`, `version`, `versioned`, `year`.
- Library crate. Keep domain, persistence, and service seams explicit.
- `postgres` feature provides env parsing, SQLx pool construction, and SQLx unit-of-work primitives.

## Ownership

- This doc rule `src/common/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, route/event shape, env vars, or child index change.
- Keep business rules here, not leaked into callers.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Service and repository split stay clean.
- Keep transport and runtime glue out of domain core.

## Verification

- `cargo check -p common`
- `cargo test -p common --all-features`

## Child DOX Index

- None.
