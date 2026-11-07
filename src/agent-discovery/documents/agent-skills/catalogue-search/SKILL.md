---
name: catalogue-search
description: Search Aura Historia's index of art, antiques and design objects offered by dealers, auction houses, marketplaces and auction platforms, and read listing details, price history, auctions and sources. Use when someone wants to find, compare or research such objects. No account is needed.
---

# Search the Aura Historia catalogue

Aura Historia is a discovery service, not a marketplace. It indexes objects that dealers,
auction houses, marketplaces and auction platforms offer on their own websites. Aura Historia
does not sell, hold stock, take bids or payments, ship or answer questions about an object.
Each listing names its source and links to the source's own page; buying, bidding and
questions happen there, under that seller's terms.

## Connect

Use either interface; both return the same catalogue data and need no credentials.

- **MCP:** `{{mcp_endpoint}}` with the Streamable HTTP transport.
- **REST:** `{{api_origin}}/v1`, described by the OpenAPI document `{{documentation_origin}}/swagger.yaml`.

## Operations

| Task | MCP tool | REST |
| --- | --- | --- |
| Search listings | `simpleSearchProductListings` | `GET /v1/product-listings` |
| Read one listing | `getProductListing`, `getProductListingByTitleSlug` | `GET /v1/product-listings/{productListingId}`, `GET /v1/product-listings/by-slug/{productListingTitleSlugId}` |
| Find similar listings | `getSimilarProductListings` | `GET /v1/product-listings/{productListingId}/similar` |
| Read a listing's history | `getProductListingHistory` | `GET /v1/product-listings/{productListingId}/history` |
| Browse auctions and their lots | `listAuctions`, `getAuction`, `getAuctionCatalogue` | `GET /v1/auctions`, `GET /v1/auctions/{auctionId}`, `GET /v1/auctions/{auctionId}/product-listings` |
| Find sources | `searchPublicListingSources`, `getPublicListingSourceBySlug` | `GET /v1/listing-sources`, `GET /v1/listing-sources/by-slug/{listingSourceSlugId}` |
| Find seller locations | `discoverPartyLocations` | `GET /v1/party-locations` |
| Read catalogue statistics | `getPublicOverview` | `GET /v1/overview` |

`language` selects the language of localized text and of search matching (default `en`);
`currency` selects the display currency (default `EUR`).

## Continue a result

Keep every other argument unchanged while you page, and stop when a response carries no
continuation.

- **Listing search:** pass the returned `searchAfter` object unchanged as the next
  `searchAfter`. On REST, send that object as one JSON-encoded query value.
- **Auctions and auction catalogues:** pass the returned `searchAfter` object serialized as a
  JSON string.
- **Sources:** pass the returned opaque `searchAfter` cursor unchanged, with the same `query`
  and `size`.
- **Seller locations:** pass the returned `searchAfter` object unchanged, with the same
  geography. On REST, send its members as `searchAfter[locationId]` and
  `searchAfter[distanceKm]`.
- **History and similar listings** return complete results and have no continuation.

A listing search page holds at most 100 items. If similar listings are not ready yet (MCP
status `EMBEDDING_PENDING`, REST `202`), ask again later.

## Report results faithfully

- Name the source and give the listing's link to the source page. Do not present Aura
  Historia as the seller.
- Availability and prices are copies of what the source published and can lag behind it.
  Tell the user to confirm both on the source page before acting.
- A display price in another currency is a conversion for orientation; the seller's own
  amount and currency are authoritative.
- Localized titles and descriptions may be machine translations; `original` holds the
  source's own text when it is available.
- Do not add condition, provenance, shipping or authenticity claims that the listing does not
  make.
