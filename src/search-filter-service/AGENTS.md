# DOX

## Purpose

- Own `search-filter-service` crate.
- Own search-filter use cases and outbound ports.

## Core Design

- Depends on `search-filter-core`, common, public ProductSearch field types from `geo`, `isocountry`, and `shop-core`, plus canonical `user-service` tier-entitlements contracts.
- Write use cases own transactions.
- Postgres and OpenSearch hidden behind ports. Create/update pass typed semantic query text to `embedding::EmbeddingGenerator::embed_search_query`; the configured provider adapter owns model-specific prompt format. Product-event matching owns its product-match prompt, response schema, typed response mapping, retry policy, and first-five-image policy; it calls the neutral generic `large-language-model::LargeLanguageModel` capability. Provider/model selection stays in runtime/provider configuration.
- Repository writes return persisted search-filter state.
- User list reads live in dedicated reader port, not repository.
- Create and update lock the authoritative user tier through transaction-scoped `UserTierEntitlements` before tier checks, active-filter counts, and writes; reactivation rechecks the stored full search and active-filter quota.
- Update generates an external embedding before the short write transaction, then revalidates the derived search state before persisting.
- Search filter timestamps live on reader/index views, not aggregates.
- CDC projection handlers reread complete Postgres index state then write through a versioned index port.
- Canonical product-event matching starts with a short service-owned source-read transaction, validates source identity and routed event kind, then skips sources whose current event ID differs from the trigger. It reports processed, duplicate, stale, missing-source, and ignored-event outcomes before percolating and evaluating enhanced filters outside PostgreSQL. Plain matches and successful enhanced matches persist in a separate short transaction even if another enhanced candidate fails. Retryable candidate failures return only after that commit so the worker retry policy can retry them; permanent failures remain explicit in the result count and never create a match.
- `GenerateSearchFilterMatchNotification` uses one service-owned PostgreSQL snapshot to read the exact `(user_id, user_search_filter_id, product_id, origin_event_id)` match, validate the lowest-filter selection rule, load the Product source, lock tier entitlements, and calculate deterministic monthly rank. Missing or mismatched match sources suppress as stale input. Its result distinguishes a new notification from deduplication. DynamoDB remains outside PostgreSQL and runs only after commit.
- Persisted-match lists compose one tie-safe match page, one factual batched Product-details read, one short PostgreSQL FX snapshot transaction (one current snapshot plus one distinct sale-snapshot batch), and one batched notification read in the service. Product presentation uses the requested Currency; notification enrichment starts after FX commit. Returned Product order follows the match page.

## Ownership

- This doc rule `src/search-filter-service/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Verification

- `cargo check -p search-filter-service`
- `cargo test -p search-filter-service --all-features`
