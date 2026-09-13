# Iteration 07 — Auction corrections

**Status: repair verified; release audit remains INCOMPLETE.** [Iteration 12](12-release-audit.md) fixed A12-01/A12-02: explicit SQL and one transaction-scoped listing advisory lock protect activation, reactivation, release, and ordinary raw/partner context decisions. F07 adds a sequence-reserved raw fence with real-PostgreSQL in-flight-capture and admission regressions. Historical successful commands below remain historical; raw/partner/admin race coverage, the broader worker matrix, and coordinated release rehearsal remain owned by the release audit.

## Objective

Add administrator ProductListing Auction-context correction, a listing-owned override barrier, and safe release.

## Delivered

- Initial-schema policy, correction-audit, release-audit, and raw-floor rows.
- One independent auction-policy version. Missing policy is version `0`.
- Full context replacement or removal. Correction requires a restricted 1–1,024 byte reason, expected listing/policy versions, expected current resolved Auction, admin authority, active listing, and same-source Auction membership.
- Correction activates the barrier even when domain facts are unchanged. Policy-only work emits no ProductListing event and does not advance listing/current/projection revisions.
- Raw context `SET` and `CLEAR` preserve the corrected context while active. Raw admission carries immutable `(stream ID, revision, generation)` identity. Release reserves one unused raw-generation sequence value as its linearization fence; captures at or before it stay blocked even when they commit late, belong to a new stream, or link after release. Linked streams additionally retain their newest pre-fence revision floor, enforced during admission. Release reads heads without locking them or raw streams; only the existing per-listing policy advisory lock serializes policy decisions. Unrelated raw facts still normalize.
- Direct typed partner context writes on an existing protected listing fail atomically.
- Admin endpoints:
  - `GET /api/v1/admin/product-listings/{productListingId}/auction-context`
  - `POST /api/v1/admin/product-listings/{productListingId}/auction-corrections`
  - `DELETE /api/v1/admin/product-listings/{productListingId}/auction-override`
- Context reads and mutations return `Cache-Control: no-store` and `ETag: "plv-{listing}-apv-{policy}"`. Correction/release require the exact strong `If-Match` value.

## Non-goals

No crawler extraction, public Auction/catalogue browsing, Auction summary hydration, Auction-ID search, Auction deletion, reoffer occurrence model, or generic policy administration.

## Verification

Passed:

```text
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::result-large-err
cargo depgraph-check check
cargo check --workspace
cargo test --workspace --lib --all-features
cargo test -p product-listing-service --all-features
cargo test -p product-service --all-features
cargo test -p product-listing-postgres --all-features
cargo test -p aura-historia-api --all-features
cargo test -p aura-historia-worker --lib --all-features
cargo test -p aura-historia-worker --test product_listing_raw_normalization --all-features
```

F07 focused verification passed:

```text
cargo check -p product-listing-service
cargo check -p product-listing-postgres
cargo check -p product-service
cargo test -p product-listing-postgres --lib --all-features product_listing_auction_override::tests::
cargo test -p product-service --lib --all-features normalize_product_listing_raw_revision::tests::
```

OpenAPI was parsed with PyYAML after correcting the Auction-context `If-Match` parameter description.

Passed after fixture/worker contract repair:

```text
cargo test -p aura-historia-worker --lib --all-features
cargo test -p aura-historia-worker --test process_durability --all-features \\
  should_persist_accepted_work_after_process_dies_before_handler_commit_t07 -- --exact
```

Passed with the authorized 20-minute bound:

```text
cargo test -p aura-historia-worker --all-features
```

The full worker suite completed successfully, including its real-infrastructure process tests.

No shared database, remote data, queue, or deployment was reset. This changes the initial business schema. A matching development checkout needs an explicitly authorized disposable-environment reset before use.
