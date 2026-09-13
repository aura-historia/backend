# Iteration 08 — crawler Auction extraction

**Status: INCOMPLETE — reopened by [iteration 12](12-release-audit.md), A12-04 and prerequisite 07.** Historical suite results below remain evidence of their tested scope. One Lot-tissimo HTML fixture and a separate synthetic raw-normalization case do not prove the required seven-source-scenario matrix or HTML-to-worker resolution. Complete that owning coverage; do not claim live source support from fixture success.

## Objective

Capture reliable source Auction evidence from crawler pages without giving crawler canonical-write authority.

## Delivered

- One Lot-tissimo-only extractor accepts only the fixture-backed HTTPS lot URL shape:
  ```text
  /{locale}/auction-catalogues/{auctioneer}/catalogue-id-{sourceAuctionId}/lot-{sourceLotId}
  ```
- It rejects unknown hosts, malformed IDs/path shapes, and query/fragment wrappers. It does not hash URLs or match names.
- The checked-in Lot-tissimo fixture now has reviewed selector evidence for `rawAuctionName` and `rawAuctionLotNumber`.
- The crawler maps this source key, catalogue URL, optional name, lot label, and nonblank date-only `lotStartDate`/`lotEndDate` into the one current raw `auction` patch. They are respectively lot opening/close assertions, not Auction schedule assertions; `Live` never becomes a lot close. A `SET` context without source key remains reliable participation but cannot resolve membership. Shared facts use `auctionMetadata`; no old raw spelling or schema discriminator was introduced.
- Crawler still only captures immutable raw observations. Unit coverage drives its current raw input through the actual raw normalizer; an isolated PostgreSQL raw-stream test captures participation first, then the fixture key/name, and proves canonical resolution after the later reliable identity. Normalization and transactional Auction resolution remain outside crawler.
- Resolver source-key locking now serializes simultaneous typed and raw first discovery/fill transactions. Real PostgreSQL races prove one Auction/discovery event and two listing attachments; the callers wait in separate transactions rather than retrying an aborted transaction.

## Non-goals

No generic URL rule, name-only resolution, source-wide timing inference, direct ProductListing/Auction writes, Auction browse/search, bids, outcomes, Party attribution, or session model. The current fixture has an empty `lotEndDate`; the mapper supports a nonblank value only as the exact labelled lot-close field, not as a broad source-time claim.

## Verification

```text
cargo fmt --all -- --check                                                        PASS
cargo depgraph-check check                                                         PASS
cargo check --workspace                                                            PASS
cargo check -p crawler                                                             PASS
cargo test -p crawler --lib scraper::auction::tests --all-features                PASS
cargo test -p crawler --lib scraper::raw_input::tests --all-features              PASS
cargo test -p crawler --test scraper_parsing_pipeline --all-features              PASS
cargo test -p crawler --all-features                                               PASS
cargo test -p product-listing-normalization --all-features                         PASS
cargo test -p product-service --all-features                                       PASS
cargo test -p product-listing-postgres --test product_listing_raw_normalization \
  --all-features should_resolve_crawler_auction_and_fill_only_absent_embedded_metadata \
  -- --exact                                                                       PASS
cargo test -p product-listing-postgres --test product_listing_raw_normalization \
  --all-features should_attach_concurrent_ -- --nocapture                         PASS
cargo test -p aura-historia-worker --lib --all-features                            PASS
cargo test -p aura-historia-worker --test process_durability --all-features \
  should_persist_accepted_work_after_process_dies_before_handler_commit_t07 -- --exact  PASS
cargo test -p aura-historia-worker --all-features                                  PASS
```

The PostgreSQL integration test uses deliberately synthetic `example.test` data, not the Lot-tissimo HTML fixture. It captures two crawler-shaped raw revisions, then drives the real normalizer and resolver. It proves one same-source Auction is linked to the listing, later conflicting name/URL/format candidates preserve initially accepted values, and an initially absent reported lot count fills.

The full worker suite completed under the authorized 20-minute bound, including its real-infrastructure process tests.

No shared database, queue, remote data, or deployment was reset. A matching development checkout needs an explicitly authorized disposable-environment reset before use.
