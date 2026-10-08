# Object IDs

Aura object IDs are typed durable identities backed by RFC 9562 UUIDv7 values. Application protocols use [TypeID v0.3](https://github.com/jetify-com/typeid/tree/main/spec):

```text
<prefix>_<26-character lowercase TypeID suffix>
```

The prefix is part of the type contract. Parsing rejects malformed/noncanonical text, wrong prefixes, non-v7 or non-RFC UUIDs, and bare UUID input. There is no legacy UUID fallback. Entity owners define concrete types; `domain-primitives` supplies the shared strict codec.

## Prefix registry

Prefixes are durable and collision-free; do not rename or reuse them.

| Type | Prefix |
| --- | --- |
| `ProductListingId` | `pl` |
| `AuctionId` | `auc` |
| `PartyId` | `pty` |
| `ListingSourceId` | `ls` |
| `UserId` | `usr` |
| `MarketingConsentSyncIntentId` | `mci` |
| `NewsletterConfirmationId` | `nsc` |
| `PartnershipId` | `psh` |
| `PartnershipApplicationId` | `pa` |
| `UserSearchFilterId` | `sf` |
| `NotificationId` | `ntf` |
| `NotificationDeliveryId` | `nd` |
| `FxRateId` | `fx` |
| `OAuthClientId` | `oc` |
| `AccessTokenId` | `at` |
| `ProductListingRawStreamId` | `prs` |
| `ProductListingRawRevisionId` | `prr` |
| `EventId` | `evt` |
| `CrawlerDomainId` | `cd` |
| `CrawlerReviewId` | `cr` |
| `CrawlerReviewPageId` | `crp` |
| `CrawlerReviewUrlId` | `cru` |

## Boundary policy

| Boundary | Representation |
| --- | --- |
| Domain and service | Concrete typed ID backed by UUIDv7. |
| `Display`, `FromStr`, serde | Canonical prefixed TypeID. |
| REST, object-ID cursor fields, semantic worker fields/keys, structured logs | TypeID. |
| OpenSearch documents and object-derived `_id` | TypeID. |
| PostgreSQL PK/FK, UUID arrays, binds and rows | Native `uuid`. |
| CDC values from PostgreSQL | UUID text, immediately mapped to typed IDs. |

PostgreSQL writes use `as_uuid()`/`into_uuid()`; reads use fallible `TryFrom<Uuid>` and reject invalid persisted identities as corruption. Never stringify a typed ID and reparse it as UUID. Storage keyset ordering and advisory-lock bytes use the backing UUID where required by their storage contract.

## Storage-only JSON

These persisted fields deliberately retain **canonical lowercase hyphenated UUID text**, not public TypeID serde:

- Partnership application proposal `listing_source_id`.
- ProductListing event `listingSourceId` and nested Auction `auctionId` references.
- ProductListing sale observation `fxRateId`.
- ProductListing enrichment `sourceEventId`.
- Notification product snapshot `listing_source_id`.
- Search-filter persisted ProductListing, ListingSource and Auction ID sets.

Adapter-local codecs explicitly encode the backing UUID and decode UUID → typed ID, validating canonical text and UUIDv7. Public history and notification DTOs still expose TypeIDs. Do not derive persisted encoding from object-ID `Display` or public serde.

Crawler `validation_summary.schema_matrix` is the exception to storage UUID text: persisted review/page references use TypeIDs, also exposed by the review API. Before a review row exists, use an absent review ID and explicit input-page index/reference, never nil or fabricated UUID placeholders.

Raw source payload, normalization context and provenance remain opaque evidence. Store Aura identities in dedicated native UUID columns unless a JSON field is explicitly documented above.

## External identity and exclusions

Cognito `sub` is an opaque provider identity, not `UserId`. Persist the verified `(issuer, subject)` separately and resolve it to an independently generated `usr_` UUIDv7. Session revocation uses that stored provider identity; never derive a subject from `UserId`.

Not every value named “ID” is an Aura object identity:

- Slugs, source/provider-controlled IDs, URLs and provider account/contact/delivery identities.
- Credentials, OAuth/exchange codes, raw bearer tokens, PKCE values, confirmation tokens/digests and session cookies.
- Request/correlation IDs, idempotency keys, ingestion submission/command IDs, queue receipt/message IDs, CDC delivery IDs/LSNs and search PIT IDs.
- Lease tokens and marketing-consent proof/fingerprint keys.

`OAuthClientId`, `AccessTokenId` and `MarketingConsentSyncIntentId` identify durable records, not their associated secrets, provider identities or operational ordering counters.

## Adding an object ID

1. Confirm it identifies a durable Aura object, not an excluded value.
2. Register a unique lowercase prefix and define the concrete type in its semantic owner.
3. Generate UUIDv7; keep raw UUID construction fallible.
4. Use native UUID storage and typed application boundaries; add any necessary storage JSON exception here explicitly.
5. Test canonical roundtrips, prefix uniqueness/assignment, wrong-prefix and bare-UUID rejection, invalid UUID versions/variants, serde and storage roundtrips.
