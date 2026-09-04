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

A Party contains only a stable ID, immutable slug, name, and optional phone/email contact. Party names trim Unicode outer whitespace, reject blank values, and allow at most 255 UTF-8 bytes without truncation. Creation derives its slug once as the slugified name plus `-<partyId>`; an empty slugification uses `party-<partyId>`. Rename and contact replacement do not alter the slug. Party rehydration validates the exact persisted slug without deriving it from the name. Party has no type, role, lifecycle, merge, or address behavior. Admins can search and explicitly create Parties at `GET`/`POST /api/v1/admin/parties`, get details at `GET /api/v1/admin/parties/{partyId}`, and update only name/contact at `PATCH /api/v1/admin/parties/{partyId}`. PATCH omits unchanged fields and clears optional contact fields with `null`; the immutable slug remains stable.

## ListingSource

A ListingSource contains a stable ID, immutable slug, name, required operator Party ID, active ingestion methods, optional presentation URL/image, and optional referral configuration. ListingSource names trim Unicode outer whitespace, reject blank values, and allow at most 255 UTF-8 bytes without truncation. Creation derives its slug once as the slugified name plus `-<listingSourceId>`; an empty slugification uses `listing-source-<listingSourceId>`. Rename does not alter the slug. Rehydration validates exact persisted name and slug. It has no type, lifecycle, search, address, crawl configuration, provider secret, or attribution policy.

ProductListing reads and projections retain the raw source URL and derive a separate outbound view URL from the current ListingSource referral configuration: Partnerize when configured, otherwise Aura UTM parameters.

Supported ingestion codes are exact canonical values:

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
POST  /api/v1/admin/listing-sources
GET   /api/v1/listing-sources/{listingSourceId}
PATCH /api/v1/listing-sources/{listingSourceId}
GET   /api/v1/listing-sources/by-slug/{listingSourceSlugId}
GET   /api/v1/me/listing-sources
GET   /api/v1/admin/listing-sources
```

Create uses an explicit operator input: `EXISTING` carries `partyId`; `NEW` carries Party name and optional contact. Admins can create ListingSources through `POST /api/v1/admin/listing-sources`, and the response includes the stable identity plus a `Location` for the admin detail resource. Admins can search Party summaries, create Parties through `GET`/`POST /api/v1/admin/parties`, get details through `GET /api/v1/admin/parties/{partyId}`, and update name/contact through `PATCH /api/v1/admin/parties/{partyId}`. Search uses bounded cursor pagination and name/contact filters; create, detail, and update return the stable identity and immutable slug. Admins can search ListingSources at `GET /api/v1/admin/listing-sources` with bounded cursor pagination, text/name, operator Party ID, ingestion-method, and exact ID/slug filters; the response contains only safe source, operator, presentation, and referral summary fields. There is no unbounded ListingSource list-all route. Public contract details are in `docs/swagger.yaml`.

## Boundaries

ProductListing stores source identity only. It has no seller, auctioneer, Party attribution, address, or location state. The crawler identifies ListingSource and SourceListing only; it never creates/resolves a Party or determines attribution.

Deferred work:

- #1646 owns durable raw ingested values.
- #1321 owns source actor resolution, attribution, and any Party merge.
- #1635 owns address/location modelling.
- #1649 owns richer OpenSearch denormalization.
