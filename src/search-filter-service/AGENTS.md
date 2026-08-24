# DOX

## Purpose

- Own `search-filter-service` crate.
- Own search-filter use cases and outbound ports.

## Core Design

- Depends on `search-filter-core`, owning core identifiers and values, pure `money`/`localization` values, shared `application` contracts, canonical Product identifiers/state/lifecycle from `product-listing-core`, public ProductSearch field types from `geo`, `isocountry`, and `shop-core`, plus canonical `user-service` tier-entitlements contracts.
- Write use cases own transactions.
- Postgres and OpenSearch hidden behind ports. Create/update pass typed semantic query text to `embedding::EmbeddingGenerator::embed_search_query`; the configured provider adapter owns model-specific prompt format. The crate-private shared Product match evaluator owns the enhanced-match prompt, response schema, typed response mapping, retry classification, ordered batch mapping, bounded concurrency, and first-five-image policy; matching use cases call the neutral generic `large-language-model::LargeLanguageModel` capability. Provider/model selection stays in runtime/provider configuration.
- Repository writes return persisted search-filter state.
- User list reads live in dedicated reader port, not repository.
- Create and update lock the authoritative user tier through transaction-scoped `UserTierEntitlements` before tier checks, active-filter counts, and writes; reactivation rechecks the stored full search and active-filter quota.
- Update generates an external embedding before the short write transaction, then revalidates the derived search state before persisting.
- `RunPeriodicSearchFilterMatching` uses focused ports for run locking, window-end candidate pages, dedupe, source reads, final filter-version/activity and Product-revision guards, idempotent writes, and separate progress; ordinary Search Filter views do not expose progress. Its short final transaction locks and exactly revalidates selected progress before any match insert or checkpoint. Checkpoints only advance; filters already covering a window are no-ops. Transient filter-local reads/writes retry with bounded delay, while malformed persisted state is terminal and isolated.
- CDC projection handlers reread complete Postgres index state and write it with only its authoritative source version. Saved-filter price ranges stay in their requested currency; Product-event percolation supplies FX-valued temporary Product prices.
- Canonical product-event matching starts with a short service-owned source-read transaction, validates source identity and routed event kind, then skips sources whose current event ID differs from the trigger. For a main source price, it loads the exact sale snapshot or latest persisted snapshot at or before immutable origin event time, converts every supported currency with checked HalfUp arithmetic, and passes only an application-owned temporary percolation input to OpenSearch. Price-bearing matches persist `EVENT` or `SALE` snapshot provenance; non-price matches persist none. It reports processed, duplicate, stale, missing-source, and ignored-event outcomes before percolating and evaluating enhanced filters outside PostgreSQL. The final short match transaction locks and rechecks the Product event ID before candidate reads and inserts, so a stale event cannot claim an idempotent match row. Plain matches and successful enhanced matches persist there even if another enhanced candidate fails. Retryable candidate failures return only after that commit so the worker retry policy can retry them; permanent failures remain explicit in the result count and never create a match.
- `GenerateSearchFilterMatchNotification` uses one service-owned PostgreSQL snapshot to read the exact `(user_id, user_search_filter_id, product_id, origin_event_id)` match, load the Product source, lock and recheck the Product revision, lock tier entitlements, and calculate monthly quota rank. Missing or mismatched match sources and stale Product events suppress as successful input handling. Each matching filter creates its own idempotent PostgreSQL notification; results distinguish a new notification from deduplication.
- Persisted-match lists compose one tie-safe match page, one factual batched Product-details read that returns canonical `ProductUserState` including notification state, and one short PostgreSQL FX snapshot transaction (one current snapshot plus one distinct sale-snapshot batch). Product presentation uses the requested Currency. Returned Product order follows the match page.

## Ownership

- This doc rule `src/search-filter-service/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Verification

- `cargo check -p search-filter-service`
- `cargo test -p search-filter-service --all-features`
