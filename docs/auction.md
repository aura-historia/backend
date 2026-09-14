# Auctions

**Status:** standalone Auction administration and public reads are available. Partners associate a ProductListing with an existing same-source Auction by internal `auctionId`.

## Scope

An Auction is Aura's source-scoped record of one sale occasion. It supports cataloguing and public reads. It is not an execution platform: bids, winners, payments, results, venue, sessions, global merge, physical-object deduplication, and reminders are out of scope.

## Identity

`auction-core` owns `AuctionId` (`auc_` strict UUIDv7 TypeID). PostgreSQL stores its backing UUID.

An Auction key is:

```text
(ListingSourceId, SourceAuctionId)
```

`SourceAuctionId` is a trimmed, opaque source value: 1–512 UTF-8 bytes after outer Unicode-whitespace trimming, no NUL, and exact preservation of case, punctuation, and internal whitespace. It is unique only within its ListingSource. Names, schedules, URLs, lot labels, Parties, and source operators are not key parts.

## Persistence and administration

PostgreSQL owns standalone source-scoped Auctions:

- `auctions` has immutable `(listing_source_id, source_auction_id)` uniqueness, a root optimistic-lock version, a restrictive ListingSource foreign key, and optional localized metadata;
- `auctions` holds optional exact UTC instants for each schedule role;
- `auction_events` records immutable `AUCTION_DISCOVERED` and `AUCTION_CHANGED` payloads. It has no CDC or worker consumer.

Administrators use `POST /api/v1/admin/auctions` and `GET`/`PATCH /api/v1/admin/auctions/{auctionId}`. Create requires `listingSourceId` and `sourceAuctionId`; duplicate source keys return `409 CONFLICT`. GET/PATCH require strict `auc_` TypeIDs. PATCH requires a positive `expectedVersion`; omitted fields remain unchanged and documented nullable fields clear with `null`. A retained Auction blocks ListingSource deletion. There is no Auction deletion endpoint.

## ProductListing association

A ProductListing may have no Auction context, an asserted lot context without membership, or membership in one same-source Auction. The context holds optional `auctionId`, opaque `lotNumber`, one-based `cataloguePosition`, and qualified lot timing.

Partner writes use `auction.auctionId` only to associate a listing with an existing same-source Auction. Omit it to preserve membership; send `null` to clear membership; send an `auc_` ID to set membership. Partner writes cannot create Auctions or change Auction metadata. Omitted lot/timing leaves preserve current values; nullable leaves clear with `null`. Raw input has no Auction fields, and raw normalization preserves stored Auction context.

## Time semantics

Auction schedule roles (`biddingOpens`, `liveStarts`, `lotsBeginClosing`, and `scheduledEnd`) are optional direct RFC3339 exact instants, for example `"liveStarts": "2026-10-18T16:03:00Z"`. Source dates and source timezones are not retained for Auctions. Lot roles remain separate: bidding opens, scheduled closes, and exact reported closure.

## Reads and boundaries

Public browsing is PostgreSQL-backed: `GET /api/v1/auctions`, `GET /api/v1/auctions/{auctionId}`, and `GET /api/v1/auctions/{auctionId}/product-listings`. Reads use `Cache-Control: no-store`. The directory is newest-first and has scoped cursors; the catalogue returns visible active assigned listings ordered by `cataloguePosition ASC NULLS LAST` then listing UUID.

ProductListing detail, search, similar-listing, and watchlist reads batch current resolved Auction summaries from PostgreSQL. Public data never exposes `sourceAuctionId` or persistence versions. Search supports exact resolved `auctionId` membership only; no Auction OpenSearch index or metadata fan-out exists.
