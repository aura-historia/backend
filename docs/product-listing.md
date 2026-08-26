# ProductListing domain contract

## Scope

Aura owns `ProductListing`, not `Product`. `Product` remains reserved for a future intrinsic/catalog identity. This rewrite is breaking and pre-production: no aliases, forwarding crates, compatibility routes, dual writes, migration/backfill machinery, or legacy OpenSearch aliases.

Provider-facing types may retain provider vocabulary, such as `ShopifyProductPayload`, WooCommerce topics, schema.org `Product` and `ItemAvailability`. Each provider boundary maps explicitly to Aura listing commands and values. Provider terms do not enter `product-listing-core`.

## Ubiquitous language

| Concept | Canonical name |
| --- | --- |
| Aggregate | `ProductListing` |
| IDs | `ProductListingId`, `ProductListingSlugId`, `ShopListingId`, `ProductListingKey` |
| Address, pricing, auction, image | `ProductListingAddress`, `ProductListingPricing`, `ProductListingAuction`, `ProductListingImage` |
| Availability | `ListingAvailability` |
| Derived availability class | `ListingOrderability` |
| Catalog membership | `ListingLifecycle` |
| Explicit sold evidence | `ListingSaleObservation` |
| Search/read types | `ProductListingSearch`, `ProductListingSummary`, `ProductListingDetails*` |
| Events/repository/document | `ProductListingEvent*`, `ProductListingRepository`, `ProductListingDocument` |

Canonical crate family:

```text
product-listing-core
product-listing-service
product-listing-postgres
product-listing-opensearch
product-listing-translation-llm
```

Dependency direction remains:

```text
product-listing-core
        ▲
product-listing-service
        ▲
product-listing-postgres / product-listing-opensearch
        ▲
runtime, API, worker, crawler, provider adapters
```

## Aggregate and invariants

`ProductListing` has private fields. Its durable state includes listing identity and mutable listing facts, plus:

```rust
availability: Option<ListingAvailability>,
lifecycle: ListingLifecycle,
sale_observation: Option<ListingSaleObservation>,
pending_event_payloads: Vec<ProductListingEventPayload>,
```

New listings are explicitly `Active`; `NewProductListing` does not accept lifecycle. `RehydratedProductListingState` is a `#[doc(hidden)] pub` adapter boundary that validates state and emits no events.

Required invariants:

1. `Withdrawn` implies `availability == None`.
2. `Active` may have a concrete availability or no assertion.
3. A sale observation is complete or absent.
4. A sale observation neither implies nor requires a lifecycle or availability value.
5. `SoldOut` neither implies nor requires a sale observation.
6. Ordinary listing-data mutation requires `Active`.
7. Withdrawal clears availability and retains sale observation.
8. Restore produces `Active` with no availability assertion.
9. Only explicitly named restore/upsert intent can restore a withdrawn listing.
10. Idempotent no-ops emit no event.

Core is deterministic: it collects pure payloads and never creates event IDs or reads the clock. Service stamps payloads with `EventId` and occurrence time before transactional persistence.

## Availability and orderability

`ListingAvailability` is an optional current source assertion. It is a canonical core enum with explicit exhaustive `as_str()` and exact `from_code()`. It has no `Default`, Serde, or SQLx derive.

| Variant | Code | Meaning |
| --- | --- | --- |
| `Available` | `AVAILABLE` | Source says available without finer stock detail. |
| `InStock` | `IN_STOCK` | Ordinary current stock is available. |
| `LimitedAvailability` | `LIMITED_AVAILABILITY` | Available with explicitly limited quantity/capacity. |
| `BackOrder` | `BACK_ORDER` | Order accepted for later fulfillment. |
| `MadeToOrder` | `MADE_TO_ORDER` | Prepared or produced after order. |
| `PreOrder` | `PRE_ORDER` | Order accepted before ordinary release. |
| `PreSale` | `PRE_SALE` | Explicit pre-sale semantics distinct from pre-order. |
| `Unavailable` | `UNAVAILABLE` | Source says unavailable without a precise reason. |
| `Reserved` | `RESERVED` | Temporarily held for another buyer. |
| `OutOfStock` | `OUT_OF_STOCK` | No current stock; it may return. |
| `SoldOut` | `SOLD_OUT` | Source says sold or permanently exhausted. |

`ListingOrderability` is derived only; it is never independently persisted or mutable.

| Values | Orderability code |
| --- | --- |
| `Available`, `InStock`, `LimitedAvailability` | `ORDERABLE_NOW` |
| `BackOrder`, `MadeToOrder`, `PreOrder`, `PreSale` | `ORDERABLE_CONDITIONALLY` |
| `Unavailable`, `Reserved`, `OutOfStock`, `SoldOut` | `NOT_ORDERABLE` |

## Lifecycle and absence semantics

```rust
pub enum ListingLifecycle {
    Active,
    Withdrawn,
}
```

Codes are `ACTIVE` and `WITHDRAWN`. Withdrawal means retained history for a listing no longer offered/published by its authoritative source. It is reversible. It is not physical purge, legal deletion, or retention cleanup.

`None` for aggregate availability has one meaning only: Aura has no sufficiently reliable current availability assertion for this active listing. `None` for `ProductListingPricing.price` means Aura has no current numeric source price recorded. Neither means unchanged. Application patches use `PatchField::{Unchanged, Set, Clear}` for that separate instruction; a price clear emits the ordinary price-change event with old `Some(price)` and new `None`.

The old canonical `ProductState` vocabulary is deleted. In particular, `LISTED`, `UNKNOWN`, `REMOVED`, and `SOLD` are not Aura listing availability values. Boundary uncertainty remains adapter-local.

## Sale observation

```rust
pub struct ListingSaleObservation {
    observed_at: OffsetDateTime,
    fx_rate_id: FxRateId,
}
```

It records when Aura first recorded an explicit sold assertion and pins the FX snapshot used to value the last advertised source price. It does not claim a completed transaction or transaction amount.

Availability writes never create, overwrite, or clear an observation. Recording an equal observation is a no-op; a different observation conflicts and requires correction. Retraction is a dedicated correction operation. `SoldOut` without an observation is valid, and a retained observation may remain after withdrawal or relisting.

Use observation FX for presentation only while currently `SoldOut`, or for a deliberately historical/withdrawn presentation. An active relisted listing uses current FX.

## Behaviors and events

Canonical aggregate behaviors are `set_price`, `clear_price`, `set_availability`, `clear_availability`, `withdraw`, `restore`, `record_sale_observation`, and `retract_sale_observation`. Old `mark_*`, generic state transition, and state-machine methods are removed.

Canonical event codes:

```text
PRODUCT_LISTING_CREATED
PRODUCT_LISTING_AVAILABILITY_CHANGED
PRODUCT_LISTING_ADDRESS_CHANGED
PRODUCT_LISTING_PRICE_CHANGED
PRODUCT_LISTING_URL_CHANGED
PRODUCT_LISTING_IMAGES_CHANGED
PRODUCT_LISTING_AUCTION_CHANGED
PRODUCT_LISTING_WITHDRAWN
PRODUCT_LISTING_RESTORED
PRODUCT_LISTING_SALE_OBSERVED
PRODUCT_LISTING_SALE_OBSERVATION_RETRACTED
```

Availability-change events contain optional previous/current availability. Withdrawal records previous availability. Event payload enum values persist canonical codes, not Rust debug output.

## Application, API, and search contracts

Create accepts `Option<ListingAvailability>`; omitted and JSON `null` create an active listing without an assertion. Update and upsert use tri-state patches for availability, main price, each price estimate, and each auction timestamp: omitted is unchanged, `null` clears, and a value sets. On creation, omitted and `null` both create no value. Upsert images are separate: omitted preserves existing images, `[]` clears them, and `null` is invalid. URL is non-clearable: omitted or `null` preserves it; a URL value sets it. Existing withdrawn listings are restored by explicit upsert intent before current facts are applied.

Withdrawal replaces normal deletion. The HTTP partner route may remain `DELETE`, but invokes `WithdrawProductListingUseCase`. Recording a sale observation is a dedicated, authorized PostgreSQL transaction that loads the aggregate and the latest FX snapshot at or before `observed_at`.

`title`, `description`, and listing address are creation-only upsert inputs. For an existing listing, they preserve current state and emit no current-state history event. Responses always emit `"availability": null` when absent. Requests parse availability tri-state. Aura route and identifier vocabulary uses `product-listings`, `productListingId`, `productListingSlugId`, and `shopListingId`.

Public listing discovery contains active listings only. Withdrawn listings are not found by public detail and are deleted from the OpenSearch projection; restore rebuilds the projection. Public discovery does not expose a lifecycle filter.

`ListingAvailabilityQuery` supports exact availability values, derived orderability values, and `include_unspecified`. Exact values OR together; orderability expands to detailed values; supplying both intersects them; unspecified values only match the missing field and are optionally ORed in. Contradictory exact/orderability filters yield no concrete matches.

OpenSearch stores an active listing document with optional availability. Concrete availability serializes as its canonical code; absent availability omits the field. Missing availability queries use `must_not exists`; `UNKNOWN` is never indexed.

## Source anti-corruption rules

Crawler normalization is boundary-local:

```text
Availability(value) — reliable assertion; set it
NoAssertion          — successful full page has no assertion; clear it
Ignore               — ambiguous/failed extraction; preserve current value
```

Reusable mappings persist `AVAILABILITY` with a non-null valid value or `NO_ASSERTION` with a null value. `Ignore` is not persisted. Presence is independent: reliable source removal becomes `Withdrawn`; timeouts, 5xx, parsing failure, blocking, and ambiguity never withdraw.

- schema.org directly maps supported availability meanings; `OnlineOnly`, `InStoreOnly`, and `Discontinued` remain adapter diagnostics/raw attributes and map to `NoAssertion` or `Ignore` by confidence.
- Shopify active with tracked inventory above zero sets `InStock`; all known tracked inventory at or below zero sets `OutOfStock`; missing/untracked inventory clears availability. Archived, draft, and delete evidence withdraw existing listings; draft does not create. Missing inventory is never zero and zero inventory is never `SoldOut`.
- WooCommerce published `instock`, `outofstock`, and `onbackorder` map to `InStock`, `OutOfStock`, and `BackOrder`. Trash/delete and nonpublished draft/pending/private evidence withdraw existing listings and do not create. Unsupported/missing status is non-destructive.
- Explicit crawler sold evidence may set `SoldOut`, but creates a sale observation only through the dedicated observation use case when that feature is required.

## Persistence contract

The initial schema uses `product_listings`, `product_listing_events`, `product_listing_translations`, and `product_listing_watchlist`; IDs use `product_listing_id`, `product_listing_slug_id`, and `shop_listing_id`.

Authoritative listing columns are nullable `availability`, non-null `lifecycle`, and the paired nullable `sale_observation_fx_rate_id` / `sale_observed_at`. PostgreSQL validates exact codes, `Withdrawn => availability IS NULL`, and the sale-observation pair. PostgreSQL is authoritative; OpenSearch is rebuildable.

Rows keep persisted enum text as `String` and map using fallible exact canonical parsing. Invalid or noncanonical persisted values are rejected; no mapping defaults or case-normalizes corrupt state.
