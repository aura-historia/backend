# API contract changelog

Only document REST API contract changes here. No internal changes.

## 2026-10-05 — Remove saved-search match explanations (#1847)

- Removed `matchReason` from ProductListing user-state responses and `enhancedMatchReason` from saved-search match records. Match feedback is unchanged; classifier scores and generated explanations are not persisted or returned.

## 2026-10-03 — Partner-managed provider ingestion configuration (#1929)

- Added idempotent provider-specific `PUT /api/v1/listing-sources/{listingSourceId}/ingestion-configurations/woocommerce` and `/shopify` endpoints. They atomically enable or fully replace only that provider configuration and return bodyless `201` for first enable or `204` for replacement/no-op, always with `Cache-Control: no-store`.
- Both routes require Partnership write access to the ListingSource. Cognito users use their normal authenticated identity; delegated Aura access tokens also require `listing-sources:write`. Integrations that also submit ProductListings need both `listing-sources:write` and `product-listings:write`.
- WooCommerce `webhookSecret` is required, nonblank, write-only, preserved exactly for HMAC verification and never returned. Currency/language are optional and omitted or `null` values clear them. Shopify `domain` remains validated and unique.
- Admin ListingSource create and configuration replacement carry `webhookSecret` inside the WooCommerce ingestion configuration object. The previous top-level `woocommerceWebhookSecret` field is removed. During admin PATCH, omitting the nested secret preserves the existing secret; supplying a string rotates it, while `null` is invalid.

## 2026-10-02 — Secret-free OAuth consent metadata read (#1926)

- Added `GET /api/v1/oauth/clients/{clientId}` for signed-in users to read persisted OAuth registration metadata before consent. It requires a Cognito access JWT (`BearerAuth`) and does not require the ADMIN role, client ownership, an existing client token, or a source partnership; Aura opaque tokens and Cognito ID tokens are rejected.
- The response uses the dedicated `OAuthClientConsentMetadataData` schema with the canonical OAuthClient ID, registration URLs, exact registered redirect URIs, and allowed scope enum. It never includes client secrets, hashes, timestamps, or admin/audit fields; an empty allowed-scope set is returned as `[]`.
- Success and errors return `Cache-Control: no-store`. The route remains behind the generic CloudFront `CachingDisabled` API behavior. This metadata read does not grant consent or issue codes or tokens; `/oauth/authorize` continues to validate redirects, scopes, and S256 PKCE.

## 2026-10-01 — Selective anonymous discovery caching (#1827)

- Enabled shared CloudFront caching only for the ten reviewed anonymous discovery GET representations: ListingSource search/detail (300 s), Auction directory/detail/catalogue (60 s), ProductListing search (60 s), detail by ID/slug (120 s), history (300 s), and Ready similar results (300 s). Only successful anonymous `200` responses are shared. Cache-enabled discovery requests carrying `Authorization`, unmarked responses without an explicit `no-store` directive, and unapproved cache directives fail closed to `private, no-store`. Unrelated handlers that already explicitly return `no-store` retain that exact header contract and remain behind `CachingDisabled`.
- The pay-as-you-go custom cache key varies on `Authorization`, `Origin`, `Host`, and all query strings, with no cookies. The default and generic `/api/*` behaviors remain caching-disabled; OPTIONS is not cached. Cognito/Aura multi-auth continues to be validated in Axum without edge token parsing or authorizer changes.
- Anonymous shared responses use `Cache-Control: public, max-age=0, s-maxage=<route TTL>, stale-if-error=0` with the TTLs listed above. No positive stale window or `stale-while-revalidate` is used; `stale-if-error=0` prevents expired successful objects from being served when the origin is unavailable or returns 5xx.
- The cache-enabled behaviors remove `X-Request-Id` and `X-Correlation-Id` from viewer responses so shared objects do not replay origin request IDs. CloudFront error caching minimum TTL is zero for the configured API error statuses.

## 2026-10-01 — Collection response sizes (#1906)

- Collection response `size` now reports the number of items in the returned `items` array, including `0` for an empty page. The `size` query parameter still controls the requested page limit.

## 2026-10-01 — Ignore WooCommerce webhook descriptions (#1920)

- WooCommerce `description` and `short_description` remain accepted provider fields but no longer update Aura's canonical product description and are omitted from persisted raw source evidence. The provider-receipt evidence digest is based on that sanitized payload; HMAC verification continues to use the exact untouched request bytes.

## 2026-10-01 — Optional partner product-listing create text (#1916)

- Partner `CreateProductListingData` no longer requires `title` or `description`; both may be omitted or sent as `null`, in line with upsert, service commands, and the ProductListing domain model. Sync and async create share this contract.

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
