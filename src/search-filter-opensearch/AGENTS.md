# DOX

## Purpose

- Own `search-filter-opensearch` crate.
- Own OpenSearch adapter for canonical search-filter index port.

## Core Design

- Writes canonical documents directly through `user_search_filters`; structural documents own index shape while local codecs preserve legacy values for canonical semantic leaves.
- Product search documents store `availability` with canonical `ListingAvailability` codes; state and lifecycle query fields are absent.
- Builds percolator queries from complete authoritative SearchFilter state using the public ProductListing percolator JSON builder. Price ranges target private `priceByCurrency.<currency>` fields and carry no FX metadata. Percolation receives only an application-owned event-time input with closed-world currency values from the service; it has no FX repository or selection policy.
- Uses Postgres `version` as OpenSearch external versioning; stale or duplicate writes are no-op outcomes.
- Vertex AI product matching is a service-orchestrated use of the neutral `large-language-model` capability, not part of this OpenSearch adapter.
- Persists every ProductListingSearch field and rejects incomplete or unknown persisted search payloads. Periodic matching progress is operational PostgreSQL state and never enters an OpenSearch document.
- Percolates complete deterministic result sets through a PIT with a bounded page size, stable `userSearchFilterId` sort, exact totals, and defensive ID deduplication; it fails instead of truncating or accepting partial results.
- Uses the shared application cursor default (currently 21) when a search-filter index query omits a cursor, never OpenSearch's implicit page size. Query sorting uses only persisted document fields; periodic progress and removed legacy checkpoints never enter the document or mapping.
- Keeps OpenSearch documents private; generic response envelopes come from `platform-opensearch`. Percolation completeness rules stay in this adapter.

## Ownership

- This doc rule `src/search-filter-opensearch/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Work Guidance

- ProductListing percolator JSON may cross from `product-listing-opensearch`; ProductListing document types may not.
- Preserve complete ProductListingSearch round-trip and percolator tests.

## Verification

- `cargo check -p search-filter-opensearch`
- `cargo test -p search-filter-opensearch --all-features`
