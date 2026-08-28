# Party and ListingSource

## Purpose

Aura separates real-world operators from listing namespaces.

```text
Party
  operates
ListingSource
  identifies
ProductListing
```

`Party` is the canonical actor. `ListingSource` is the source-local namespace where a `SourceListingId` is meaningful. A ProductListing identity is `(ListingSourceId, SourceListingId)`.

## Party

A Party contains only a stable ID, immutable slug, name, and optional phone/email contact. Creation derives its slug once. Rename and contact replacement do not alter the slug. Party has no type, role, lifecycle, merge, address, or REST API.

## ListingSource

A ListingSource contains a stable ID, immutable slug, name, required operator Party ID, active acquisition methods, optional presentation URL/image, and optional referral configuration. It has no type, lifecycle, search, address, crawl configuration, provider secret, or attribution policy.

Supported acquisition codes are exact canonical values:

```text
WEB_CRAWL
SHOPIFY
WOOCOMMERCE
PARTNER_API
```

Provider configuration belongs to ListingSource service/PostgreSQL adapters. The business database records only that `WEB_CRAWL` is active; crawler domains, schedules, retries, schemas, budgets, and review artifacts belong to crawler-local PostgreSQL.

## Partnership

A Partnership is the active business relationship for one Party. Membership and ListingSource access are relational state:

```text
partnership_members(user_id, partnership_id)
partnership_listing_source_grants(partnership_id, listing_source_id)
```

A ProductListing partner write requires both membership and a ListingSource grant through the same Partnership. PartnershipApplication approval creates any proposed Party/ListingSource, finds or creates the Party Partnership, adds the applicant membership, grants the ListingSource, updates the application, and creates its notification in one PostgreSQL transaction.

## API

ListingSource is the only public source resource:

```text
POST  /api/v1/listing-sources
GET   /api/v1/listing-sources/{listingSourceId}
PATCH /api/v1/listing-sources/{listingSourceId}
GET   /api/v1/listing-sources/by-slug/{listingSourceSlugId}
GET   /api/v1/me/listing-sources
```

Create uses an explicit operator input: `EXISTING` carries `partyId`; `NEW` carries Party name and optional contact. There is no Party route and no ListingSource search/list-all route. Public contract details are in `docs/swagger.yaml`.

## Boundaries

ProductListing stores source identity only. It has no seller, auctioneer, Party attribution, address, or location state. The crawler identifies ListingSource and SourceListing only; it never creates/resolves a Party or determines attribution.

Deferred work:

- #1646 owns durable raw ingested values.
- #1321 owns source actor resolution, attribution, and any Party merge.
- #1635 owns address/location modelling.
- #1649 owns richer OpenSearch denormalization.
