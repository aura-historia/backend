# API contract changelog

## 2026-09-29 — Async partner product-listing update (documentation target, #1861)

- Documented `PATCH /api/v1/listing-sources/{listingSourceId}/product-listings/async` separately from unchanged synchronous PATCH, reusing its `UpdateProductListingData` item contract and the async POST admission report, idempotency header, error envelope, and `202`/`400`/`401`/`403`/`413`/`500`/`503` status rules. Invalid patch items can fail independently; listing/Auction existence, partnership and business conflicts are decided downstream, not on admission.
- Added a partial-admission update example and unchanged-whole-batch retry guidance: keep the same key and original indices after transport uncertainty; correct definitely invalid items separately under a new key. `202` confirms queue admission, not an applied update or a status resource. This is a documentation target only; the async PATCH route and infrastructure are not enabled by this entry.

## 2026-09-29 — Async partner product-listing create (documentation target, #1860)

- Documented `POST /api/v1/listing-sources/{listingSourceId}/product-listings/async` separately from the unchanged synchronous create route. It accepts the same array of create items (up to 100), evaluates valid siblings independently, and reports confirmed queue admission with `submissionId`, `acceptedCount`, and ordered original-index item failures rather than created listing IDs.
- Documented `202` on partial or empty-batch admission, evaluated batch reports on zero-accepted `400`/`413`/`500`/`503`, and the existing `ApiError` envelope for pre-evaluation failures. An optional `Idempotency-Key` is echoed on evaluated reports; transport retries require the same key and unchanged ordered batch. Definitely invalid corrections require a new-key request. Same-group FIFO successors wait for unresolved predecessors; unrelated groups may proceed.
- Specified the front-door route authorization and CORS request/response header requirements. These are documentation-only changes; the route and infrastructure are not enabled by this entry.
