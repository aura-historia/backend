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
- `auction_events` records immutable `AUCTION_DISCOVERED` and `AUCTION_CHANGED` payloads in the same transaction as Auction state. `AUCTION_CHANGED.schedule` retains whole previous/current schedules. It has no CDC or worker consumer yet.

Administrators use `POST /api/v1/admin/auctions` and `GET`/`PATCH /api/v1/admin/auctions/{auctionId}`. Create requires `listingSourceId` and `sourceAuctionId`; duplicate source keys return `409 CONFLICT`. GET/PATCH require strict `auc_` TypeIDs. PATCH requires a positive `expectedVersion`; omitted fields remain unchanged and documented nullable fields clear with `null`. A retained Auction blocks ListingSource deletion. There is no Auction deletion endpoint.

## ProductListing association

A ProductListing owns optional Auction/lot facts: `auctionId`, opaque `lotNumber`, one-based `cataloguePosition`, and exact lot timestamps. Each fact is independent. A listing may have lot facts with no `auctionId`; that is a valid individually auctioned listing, not an unresolved or synthetic parent Auction.

Partner writes use `auction.auctionId` only to associate a listing with an existing same-source Auction. Omit it to preserve membership; send `null` to clear membership; send an `auc_` ID to set membership. Partner writes cannot create Auctions or change Auction metadata. Omitted lot/timing leaves preserve current values; nullable leaves clear with `null`. Raw input has no Auction fields, and raw normalization preserves stored Auction/lot facts.

## Time semantics

Auction schedule roles (`biddingOpens`, `liveStarts`, `lotsBeginClosing`, and `scheduledEnd`) are optional direct RFC3339 exact instants, for example `"liveStarts": "2026-10-18T16:03:00Z"`. Source dates and source timezones are not retained for Auctions. Lot roles remain separate: bidding opens, scheduled closes, and exact reported closure. An Auction schedule never fills, rewrites, or implies a listing's lot times.

## Reads and boundaries

Public browsing is PostgreSQL-backed: `GET /api/v1/auctions`, `GET /api/v1/auctions/{auctionId}`, and `GET /api/v1/auctions/{auctionId}/product-listings`. Reads use `Cache-Control: no-store`. The directory is newest-first and has scoped cursors; the catalogue returns visible active assigned listings ordered by `cataloguePosition ASC NULLS LAST` then listing UUID.

ProductListing search and similar-listing results stay lightweight: they return only the indexed `auctionId` association and never hydrate current Auction metadata. PostgreSQL full-detail, watchlist, saved-search match-detail, and Auction catalogue reads join the parent Auction presentation in their ProductListing detail read model; handlers do not run a second Auction batch lookup. Standalone lot facts remain readable without a parent. Public data never exposes `sourceAuctionId` or persistence versions. Search supports exact resolved `auctionId` membership only; no Auction OpenSearch index or metadata fan-out exists.

## Events and later notifications

Changing a lot close creates a `PRODUCT_LISTING_CHANGED` payload with its previous/current listing facts. Changing a shared schedule creates an `AUCTION_CHANGED` payload with previous/current schedules; it does not create ProductListing changes for Auction members.

Reminder and watchlist delivery are not implemented. A later consumer must durably consume these events, handle duplicate delivery and visibility/subscription rules, and use event payload snapshots rather than today's Auction row.
