# ListingSource and Partnership rewrite inventory

## Status

Iteration 6 is complete. Aura-owned listing-source, partner, and onboarding contracts use `ListingSource`, `Partnership`, and `PartnershipApplication`. Legacy source and application contracts are removed.

## Canonical boundary

- `ListingSource` owns an ingestion source operated by a `Party`.
- `Partnership` owns membership and ListingSource grants.
- `PartnershipApplication` proposes an existing ListingSource or a new Party plus ListingSource.
- `ProductListing` identity uses `listingSourceId` and `sourceListingId`.
- PostgreSQL is authoritative. ProductListing and saved-filter OpenSearch indexes are rebuildable projections; no legacy source index exists.

## Active API

- ListingSource create: `POST /api/v1/admin/listing-sources`; admin details/update: `GET`/`PATCH /api/v1/admin/listing-sources/{listingSourceId}`.
- ListingSource lookup: `/api/v1/listing-sources/by-slug/{listingSourceSlugId}`.
- Caller ListingSources: `/api/v1/me/listing-sources`.
- Partner ProductListing batches: `/api/v1/listing-sources/{listingSourceId}/product-listings`.
- WooCommerce intake: `/api/v1/webhooks/woocommerce/{listingSourceId}`.
- Applicant PartnershipApplication routes: `/api/v1/me/partnership-applications`.
- Admin PartnershipApplication routes: `/api/v1/partnership-applications`.

## Final scan checklist

- [x] Public OpenAPI routes and payloads use ListingSource and Partnership names.
- [x] Event and storage docs remove legacy source projection and application ownership.
- [x] Architecture examples and credential scopes use active bounded contexts.
- [x] The obsolete source OpenSearch mapping is removed.
- [x] Deployment publishes canonical `partnership-application` MJML templates only.
- [x] Dated CHANGELOG entries retain historical terminology.
