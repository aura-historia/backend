# API contract changelog

## 2026-09-29 — Async partner product-listing create (documentation target, #1860)

- Documented `POST /api/v1/listing-sources/{listingSourceId}/product-listings/async` separately from the unchanged synchronous create route. It accepts the same array of create items (up to 100), evaluates valid siblings independently, and reports confirmed queue admission with `submissionId`, `acceptedCount`, and ordered original-index item failures rather than created listing IDs.
- Documented `202` on partial or empty-batch admission, evaluated batch reports on zero-accepted `400`/`413`/`500`/`503`, and the existing `ApiError` envelope for pre-evaluation failures. An optional `Idempotency-Key` is echoed on evaluated reports; transport retries require the same key and unchanged ordered batch. Definitely invalid corrections require a new-key request. Same-group FIFO successors wait for unresolved predecessors; unrelated groups may proceed.
- Specified the front-door route authorization and CORS request/response header requirements. These are documentation-only changes; the route and infrastructure are not enabled by this entry.
