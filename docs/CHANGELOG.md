# API contract changelog

Only document REST API contract changes here. No internal changes.

## 2026-09-30 — Versioned health and readiness probes (#1907)

- Moved the anonymous main API probes to `GET /api/v1/health` and `GET /api/v1/ready`; removed the old `/health` and `/ready` paths. Health remains liveness-only and returns `200` with `ok\n`.
- Readiness retains its PostgreSQL and OpenSearch checks and returns `204` when ready or `503` when a dependency fails or the check times out. Responses do not expose sensitive readiness diagnostics.

## 2026-09-29 — Async partner idempotency header validation (R2)

- Async POST/PATCH/PUT/DELETE now reject a supplied `Idempotency-Key` containing a comma, including comma-joined HTTP API v2 header values, with request-level `400 BAD_HEADER_VALUE` before publication. Native duplicate headers remain invalid. The accepted grammar is 1–128 visible ASCII bytes excluding comma (`^[\x21-\x2B\x2D-\x7E]{1,128}$`); omitted keys are still generated for evaluated submissions.

## 2026-09-29 — WooCommerce webhook shared-queue admission (#1865)

- `POST /api/v1/webhooks/woocommerce/{listingSourceId}` keeps required partner bearer/capability checks, source configuration lookup, and WooCommerce HMAC over the exact untouched request bytes. Authorized ignored create/update statuses remain bodyless no-op `204` without queue submission or provider receipt, after an immediate partner/source grant check. Mapped commands check the current partner/source grant in the consumer. For mapped observations, bodyless `204` means **confirmed admission** of one `CAPTURE_RAW` command to the shared ProductListing FIFO, not raw capture or canonical completion.
- Removed immediate capture/receipt/source-order `409` responses in favor of downstream consumer failure/retry/DLQ handling. Failed, oversized, unconfirmed, or not-attempted queue admission is not acknowledged (`413`, `503`, or `500` as appropriate); a lost send reply can still mean queued work. Preserve the signed request and delivery identity on retry, and inspect FIFO/receipts before redrive. The shared consumer commits raw evidence and its command receipt later; this is not a distributed transaction with the HTTP acknowledgment.
- CDK already configures the API Lambda with the stage-local shared FIFO URL and source-ARN-only `sqs:SendMessage` permission, so no infrastructure change is required. The API and WooCommerce service now use the existing shared publisher; deploy the shared resources and compatible consumer before the API producer cutover. Source changes do not prove live deployment.

## 2026-09-29 — Async partner product-listing withdrawal front door (#1863)

- Added `DELETE /api/v1/listing-sources/{listingSourceId}/product-listings/async` to the exact Gateway route catalog with the same Partner application-bearer policy and unmodified request-body forwarding as async POST/PATCH/PUT. Axum submits individual WITHDRAW intents through the shared async admission use case; synchronous DELETE remains unchanged.
- Documented the synchronous DELETE `WithdrawProductListingData` array as the async request and reused the shared async admission report, `Idempotency-Key` header, and `202`/`400`/`401`/`403`/`413`/`500`/`503` rules. Partial admission may leave some listings unwithdrawn; `202` confirms queue custody only, not completed withdrawal or immediate search removal. Retry transport-uncertain batches unchanged with the same key and original indices; correct definitely invalid items separately with a new key.

## 2026-09-29 — Async partner product-listing upsert (#1862)

- Added `PUT /api/v1/listing-sources/{listingSourceId}/product-listings/async` to the exact API Gateway route catalog with the Partner application-bearer policy. The synchronous PUT remains unchanged.
- Documented the synchronous `UpsertProductListingData` request items and the shared async admission report, `Idempotency-Key`, error envelope, and `202`/`400`/`401`/`403`/`413`/`500`/`503` rules. `202` confirms queue admission, not an applied upsert; retry uncertain batches unchanged with the same key, and correct definitely invalid items separately with a new key. The PUT handler uses the existing synchronous upsert item converter without changing synchronous PUT.

## 2026-09-29 — Async partner product-listing update (documentation target, #1861)

- Documented `PATCH /api/v1/listing-sources/{listingSourceId}/product-listings/async` separately from unchanged synchronous PATCH, reusing its `UpdateProductListingData` item contract and the async POST admission report, idempotency header, error envelope, and `202`/`400`/`401`/`403`/`413`/`500`/`503` status rules. Invalid patch items can fail independently; listing/Auction existence, partnership and business conflicts are decided downstream, not on admission.
- Added a partial-admission update example and unchanged-whole-batch retry guidance: keep the same key and original indices after transport uncertainty; correct definitely invalid items separately under a new key. `202` confirms queue admission, not an applied update or a status resource. This is a documentation target only; the async PATCH route and infrastructure are not enabled by this entry.

## 2026-09-29 — Async partner product-listing create (documentation target, #1860)

- Documented `POST /api/v1/listing-sources/{listingSourceId}/product-listings/async` separately from the unchanged synchronous create route. It accepts the same array of create items (up to 100), evaluates valid siblings independently, and reports confirmed queue admission with `submissionId`, `acceptedCount`, and ordered original-index item failures rather than created listing IDs.
- Documented `202` on partial or empty-batch admission, evaluated batch reports on zero-accepted `400`/`413`/`500`/`503`, and the existing `ApiError` envelope for pre-evaluation failures. An optional `Idempotency-Key` is echoed on evaluated reports; transport retries require the same key and unchanged ordered batch. Definitely invalid corrections require a new-key request. Same-group FIFO successors wait for unresolved predecessors; unrelated groups may proceed.
- Specified the front-door route authorization and CORS request/response header requirements. These are documentation-only changes; the route and infrastructure are not enabled by this entry.
