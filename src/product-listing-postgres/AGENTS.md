# DOX

## Purpose

- Own `product-listing-postgres` crate.
- Own canonical Product Listing SQLx adapters for Postgres.

## Core Design

- Depends on `product-listing-core`, `product-listing-service`, `listing-source-core`, `notification-core`, `domain-primitives` versioning, `money`/`localization` canonical values, and shared `platform-postgres` UoW primitives.
- Exports public SQLx Product Listing repository, event-store, factual details, history, embedding, user-state, ListingSource-summary batch reader, batch details, batch watchlist-details, search-filter match-source reader, current-revision guard, and exact content-assessment snapshot reader factories only. Factual detail, batch-detail, watchlist-detail, and match-source readers return source pricing plus optional immutable sale observation; service owns exact-FX lookup and final pricing presentation. The match-source reader loads exact `(ProductListingId, EventId)` refs with one set-based query and exposes immutable `product_listing_events.event_time`, source event kind, and current Product Listing event ID for stale-safe percolation. The current-revision guard batch-locks requested `product_listings` rows `FOR SHARE` through final match commit. Embedding source reader and writer reread committed current Product Listing state, then lock/revalidate the source revision and atomically store vectors plus `ENRICHMENT_EMBEDDED` provenance events.
- The ordinary ListingSource-summary reader resolves unique ListingSource IDs to source ID, name, and slug in one set-based query for ProductListing search/KNN hydration. The ordinary Product Listing user-state reader resolves an OpenSearch result page in one set-based query: profile consent/tier, watchlist, selected search-filter match, Free-tier monthly hide state, and all unseen notification IDs ordered newest-first. Factual detail, batch-detail, and watchlist-detail readers return the same complete user state from their own SQL query.
- Keeps SQL rows, SQL, mappings, repositories, event stores, and reader internals private. Content-assessment source reads normalize canonical empty title or description text to absence.
- Product Listing row and `product_listing_events` append bind to caller-owned transactions through service factory ports. `product_listings.event_id` remains the generic current revision, while immutable `content_source_event_id` is initialized from `PRODUCT_LISTING_CREATED` and guards text assessments. The event store serializes the core-owned `ProductListingEventPayload` carried by the service-owned `ProductListingEvent` alias; every timestamp embedded in JSON uses RFC 3339 and the history reader strictly reconstructs that same payload from canonical event types and JSON. The Product Listing event-history reader returns domain events only.
- Product Listing rows use `listing_source_id` plus `source_listing_id`; they do not store seller or address/geo data. Material Product Listing reads join `listing_sources` for source ID, name, slug, and referral configuration in the same query; they retain raw URLs and derive outbound view URLs with `listing-source-core::outbound_url`. Product Listing `availability` is nullable canonical text; `lifecycle` is `ACTIVE` or `WITHDRAWN`, and withdrawn rows must have null availability. Source price columns contain no FX ID; paired `sale_observation_fx_rate_id` and `sale_observed_at` persist `ListingSaleObservation`. Canonical FX storage and transactional latest-snapshot reads are owned by `fxrate-postgres`.
- Batch watchlist details use a tie-safe `created DESC, product_listing_id ASC` cursor page with one joined query.
- Real Postgres integration tests live under `tests/` by implementation file, with helpers inline per file.

## Ownership

- This doc rule `src/product-listing-postgres/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- Update this file when crate contract, dependency edge, SQL shape, or factory exports change.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Keep adapter types private unless composition root needs factories.
- Map rows with `TryFrom`; never leak SQLx row types.
- Preserve SQLx and row-mapping failures as error sources in service port errors.

## Verification

- `cargo check -p product-listing-postgres`
- `cargo test -p product-listing-postgres --all-features`
- `cargo test -p product-listing-postgres --tests` runs real Postgres integration tests split by implementation file.

## Child DOX Index

- None.
