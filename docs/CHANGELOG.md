# API contract changelog

Only document REST API contract and user-visible behavior changes here. No internal changes.

## 2026-10-26 — WooCommerce variation webhooks (#2523)

- `POST /v1/webhooks/woocommerce/{listingSourceId}` treats `product.created` and `product.updated` payloads with `type: variation` as authorized ignored events whatever their `status` (bodyless `204`, no queue send, receipt or listing). They previously created a second listing keyed by the variation ID. The parent's own `product.updated`, which WooCommerce delivers after saving or deleting a variation, refreshes the parent's listing. `product.deleted` is unchanged and never withdraws a variation's parent.
- Later variation events do not withdraw a listing created from a variation ID before this change; withdraw it through the partner API.

## 2026-10-25 — Consumer-only MCP catalogue (#2481)

- **Breaking change:** MCP removes all 36 partner tools: listing management, synchronous and asynchronous listing batches, Shopify ingestion configuration, party locations, source-location defaults, shipping profiles/defaults and listing delivery. `tools/list` returns the 38 consumer operations (12 public, 26 private-user) on one page without `nextCursor`; a call naming a removed tool fails as an unknown tool (`-32602`). Partner workflows continue unchanged through REST. The endpoint, transport, OAuth contract, scopes and the retained tools' names, schemas and behavior are unchanged. See the [MCP contract](mcp.md#partner-workflows-use-rest).

## 2026-10-25 — Wildcard REST browser CORS

- REST Gateway CORS now returns `Access-Control-Allow-Origin: *` for browser origins in both stages, including ephemeral `*.bitter-frog-5102.workers.dev` previews. Allowed methods, headers, exposed headers, and all API authorization requirements are unchanged. Cross-origin callers may supply bearer tokens in `Authorization`, but `credentials: "include"` is incompatible with wildcard CORS. MCP's separate explicit origin restrictions and Cognito registered redirect URIs are unchanged.

## 2026-10-23 — Bounded listing shipping reads (#2444)

- **Breaking change:** search, similar listings, watchlist, saved-search matches and auction catalogues return a bounded `item.shipping` summary with optional `eligibility`, `requiredInputs`, `explanations` and an independent `collection` summary. `userState.shipping` adds at most one `pricePreview` (service, explicit basis and price, in the requested currency when convertible), required `attribution` and a `display` valuation only when the preview was converted. Collections no longer return terms, converted per-zone rates, options or zone/service indices, including the detail-shaped watchlist, match and catalogue items (`PersonalizedProductListingCollectionDetailsData`).
- **Breaking change:** detail by ID/slug and the watchlist update response keep effective terms in a read representation without `schema`, null assertions, dispatch origins or collection sites. A converted value appears as `display_price` beside the source `price`, with one shared `display` valuation only when a value was converted. `userState.shipping` keeps a minimal `destination` and lists self-contained `options` naming their service and rate; `charge` appears only when calculable.
- Listing reads no longer return `listingRevision`, `sourceRevision`, `defaultRevision`, `profile`, `listingEvidence`, `evaluatedAt`, `shippingReasons`, `collectionReasons`, `notices`, `reasons`, `applicability`, `selectedZone`, `serviceIndex`, `bindingQuote` or `completeLandedCost`. Absent optional values are omitted instead of serialized as null, and `item.shipping` is omitted when nothing is asserted. MCP tools share these shapes and generated output schemas. Shipping management, delivery configuration, import and write protocols are unchanged. See [shipping](shipping.md#shipping-on-listing-reads) and [OpenAPI](swagger.yaml).

## 2026-10-23 — Complete partner MCP catalogue (#2255)

- Compose the 36 selected partner tools with the existing 38 public and private-user tools in one 74-operation catalogue. Clients must follow `tools/list` continuation across both pages. Partner tools retain current delegated scopes and service-owned source/party authority, complete shared contracts, version checks and explicit partial/admission outcomes. WooCommerce secret configuration remains excluded. See the [partner manifest](https://github.com/aura-historia/backend/blob/753a3b9395da1ce4cada94fb63078f907a8ac93d/src/aura-historia-mcp/partner-operations.json) and [workflow and retry contract](https://github.com/aura-historia/backend/blob/753a3b9395da1ce4cada94fb63078f907a8ac93d/docs/mcp.md#partner-workflow).

## 2026-10-23 — Listing delivery MCP family (#2254)

- Define source-keyed delivery inspection, versioned change and single-command asynchronous submission through the existing application services. Shared contracts preserve every delivery field, public reference, import, version and outcome. Admission retains the full source-local identity and shared retry/uncertainty report without claiming committed mutation. See the [delivery tool contract](https://github.com/aura-historia/backend/blob/753a3b9395da1ce4cada94fb63078f907a8ac93d/docs/mcp.md#listing-delivery-management).

## 2026-10-23 — Partner ingestion MCP family (#2253)

- Define four asynchronous listing admission tools and non-secret Shopify configuration through the existing application use cases. Admission reports preserve original positions, effective retry keys and published/rejected/not-attempted/unknown outcomes; queue admission is distinct from committed state. Raw MCP arguments retain strict DTO duplicate-field validation. REST response bodies and key headers remain stable. WooCommerce configuration remains excluded. See the [ingestion contract](https://github.com/aura-historia/backend/blob/753a3b9395da1ce4cada94fb63078f907a8ac93d/docs/mcp.md#asynchronous-partner-ingestion-and-shopify-configuration).

## 2026-10-23 — Shared synchronous partner listing batches (#2252)

- Add the four synchronous partner batch MCP family operations with complete shared REST item contracts. Results explicitly distinguish completed, partial and failed execution and retain original indices, source IDs, successful identities and safe per-item failures. All input conversion precedes writes; items execute sequentially in independent transactions. See the [batch contract](https://github.com/aura-historia/backend/blob/753a3b9395da1ce4cada94fb63078f907a8ac93d/docs/mcp.md#synchronous-partner-listing-batches).

## 2026-10-23 — Granted listing-source pages (#2249)

- **Breaking development change:** `GET /v1/me/listing-sources` (`getMyListingSources`) returns `{ items, next? }` instead of an array. It accepts `size` (default 50, range 1–100) and opaque `after`; callers must follow `next` until absent to enumerate every eligible source. Every page rechecks current grants and delegated `listing-sources:read`; continuations are bound to the authenticated actor and stable source-name/ID ordering. The browser source selector must consume this page contract.

## 2026-10-21 — Authenticated MCP and Aura OAuth (#2228)

- The stable MCP endpoint now exposes 38 operations: the existing 12 public tools plus 9 own-account/address/destination, 7 saved-search/match-feedback, 4 watchlist and 6 notification tools. Protected families retain complete shared REST contracts, independent read/write scopes, ownership, quotas, pagination and version conflicts. Mutations preserve their existing retry semantics and are never automatically retried. See the [MCP contract](mcp.md) and [authenticated manifest](../src/aura-historia-mcp/authenticated-operations.json).
- Ten public listing, history, auction and source tools accept validated delegated context. Private listing sections follow their individual read scopes; hidden-item, tier and sensitive-image policy applies independently. Anonymous access remains available, and location discovery and public overview remain anonymous. Published integer schemas include unsigned bounds matching the existing wire types.
- MCP revalidates opaque Aura credentials, expiry, revocation and user suspension on every request. Protected calls use HTTP `401`/`403` challenges with resource metadata and complete required scope sets; invalid supplied credentials never become anonymous. Authentication-store outages return `503`. Both protected-resource metadata paths identify the exact MCP resource and matching Aura issuer.
- `GET /v1/oauth/authorize` and `POST /v1/oauth/token` accept one optional `resource` from the configured environment's exact REST/MCP identifiers. Authorization codes retain that binding; exchange may repeat it or omit it to inherit the binding. Foreign, other-environment, empty, repeated or substituted resources return `400 invalid_target`. Existing integrations omitting `resource` retain the shared Aura default, and opaque tokens remain usable across both transports with their existing scopes and revocation.
- Public RFC 8414 metadata at `/.well-known/oauth-authorization-server` advertises the matching browser consent URL, canonical `/v1/oauth/token`, authorization code, S256 and `client_secret_post`. It omits refresh grants, dynamic registration, CIMD, revocation/introspection discovery and authorization-response `iss` support. First-party approval still defaults to nonexpiring tokens, which omit `expires_in`; no token migration or refresh token is introduced.
- OAuth authorization input and token/revocation/introspection failures use standard, secret-free OAuth JSON errors. Invalid clients return `401 invalid_client`; unknown authorization clients return `400 invalid_request`. Duplicate parameters and malformed form/query encoding are rejected. Credential responses, redirects and protocol errors carry `Cache-Control: no-store` and `Pragma: no-cache`. REST client-management and first-party authentication/authority errors retain their existing vocabulary. See the [OAuth contract](oauth.md).

## 2026-10-20 — Complete listing-search filter documentation (#2235)

- OpenAPI now documents the existing `orderability` and `includeUnspecifiedAvailability` query filters on `GET /v1/product-listings`. They retain the same availability-query semantics as saved searches.

## 2026-10-20 — Anonymous MCP catalogue (#2239)

- Add twelve anonymous listing, history, auction, source, location and overview tools at the canonical MCP endpoint, using the existing operation IDs and complete shared contracts. Search retains its full filters and continuations; listing history remains complete and unpaginated. Newsletter, private/write/admin and control-plane operations are excluded. See the [public MCP contract](mcp.md#public-catalogue) and [selected-operation manifest](../src/aura-historia-mcp/public-operations.json).

## 2026-10-20 — Delegated credentials cannot obtain OAuth credentials (#2214)

- `GET /v1/oauth/authorize` now requires a Cognito access JWT. A valid Aura Historia access token, including one with `access-tokens:write` and every requested scope, receives `403 FORBIDDEN` and no authorization code is issued. Previously such a token could approve a request and obtain a non-expiring OAuth token that outlived it. Approvals made with a Cognito access JWT and token issuance at `POST /v1/oauth/token` are unchanged.
- `POST /v1/admin/oauth-clients` now requires a Cognito access JWT because its response contains a new client secret. An administrator's Aura Historia access token receives `403 FORBIDDEN` and no client is registered. OpenAPI no longer lists `AccessTokenAuth` for this operation. Reading, updating and deleting OAuth clients keep accepting administrators' Aura Historia access tokens with the existing scopes.

## 2026-10-20 — REST base path development cutover (#2216)

- **Breaking development change:** all REST operations now use `/v1/...` on `https://api.aura-historia.com` and `https://api.stage.aura-historia.com`, replacing `/api/v1/...`. The old routes are removed without aliases, redirects or a deprecation period. Generated `Location` headers use `/v1/...`; operation IDs, authentication, request/response schemas and operation/retry semantics are unchanged.
- Update backend callers (including webapp OAuth consent approval), advertised Aura OAuth authorization/token endpoints, and registered WooCommerce/Loops HTTP webhook destinations together. Client callback URLs, webapp-owned `/api/...` routes and third-party provider API URLs are independent and must not be rewritten unless they actually target a renamed backend route. See the [front-door cutover requirements](http-api-front-door.md#rest-client-and-provider-configuration).
- Earlier entries retain the paths that applied when they were published.

## 2026-10-20 — Scoped personalization for delegated tokens (#2213)

- Aura Historia access tokens now receive each `userState` section only with its read scope: `watchlist` needs `watchlist:read`, `notification` needs `notifications:read`, `searchFilter` needs `search-filters:read`, and `contentVisibility` needs `users:read`. This applies to product-listing detail by ID and slug, search, similar listings, auction catalogues, watchlists and saved-search matches. These tokens previously received every section regardless of scope.
- Sections without their scope are omitted rather than returned as `false` or empty values. `userState` is omitted when no section applies. `ProductListingUserStateData` therefore no longer requires `watchlist`, `contentVisibility`, `notification` or `searchFilter`; regenerate clients that assumed them. `userState.shipping` keeps its existing `users:read` requirement.
- `PATCH /api/v1/me/watchlist/{productListingId}` still needs only `watchlist:write` to apply the update, but its response follows the same read scopes: the updated `userState.watchlist` now also requires `watchlist:read`.
- Undisclosed state still applies to the listing: matches hidden by the saved-search quota remain redacted, and the stored content preference still decides image visibility. Cognito users and anonymous callers are unaffected.

## 2026-10-20 — Delegated access-token management cannot expand authority (#2212)

- `POST /api/v1/me/access-tokens` now requires a Cognito access JWT. A valid Aura Historia access token, including one with `access-tokens:write`, receives `403 FORBIDDEN` and no token is created. OpenAPI no longer lists `AccessTokenAuth` for this operation. Integrations that minted tokens with an Aura access token must create them while signed in instead.
- `PATCH /api/v1/me/access-tokens` called with an Aura Historia access token may still rename a token, narrow its `scopes` to a subset of the current ones (including `[]`) and set `expires` no later than the current expiry, or to any time when the token has none. Adding a scope, extending the expiry, or sending `expires: null` for a token that has an expiry returns `403 FORBIDDEN` and leaves the token unchanged. This includes the bearer token itself. Cognito access JWT callers are unaffected, and `DELETE` keeps its existing rules.

## 2026-10-19 — Loopback OAuth redirect URIs at the edge (#2206)

- CloudFront no longer applies its EC2 metadata SSRF and IPv4 remote-file-inclusion request checks to OAuth client registration and update (`POST /api/v1/admin/oauth-clients`, `PATCH /api/v1/admin/oauth-clients/{client_id}`), `GET /api/v1/oauth/authorize` or `POST /api/v1/oauth/token`. Contract-valid loopback HTTPS redirect URIs, such as `https://localhost:3458/oauth/callback` used by Shopify CLI `--use-localhost` development, therefore reach the API's redirect URI validation. Registering that URI previously failed with an HTML `403` (`Request blocked.`) before authentication or validation. Redirect URI validation is unchanged, and other routes keep these checks.

## 2026-10-19 — OAuth credential errors no longer echo input (#2207)

- `POST /api/v1/oauth/token`, `/introspect` and `/revoke` no longer include the submitted value in the problem `detail` when `client_secret` or `token` is malformed, for example a mistyped value or an access token sent as `client_secret`. Status and error codes are unchanged: a malformed `client_secret` returns `401 INVALID_CREDENTIALS`, a malformed `token` returns `400 BAD_BODY_VALUE`. Both now identify the form field in `source` and use a fixed `detail`. Clients that parsed the old message should rely on `error` and `source.field` instead.

## 2026-10-19 — Normalized listing title typography (#2201)

- ProductListing titles in listing, watchlist, search and history responses no longer contain decorative typography. Styled letters and digits such as `𝐄𝐢𝐧`, `Ⓥⓐⓢⓔ` or `ＶＡＳＥ` are returned as ordinary text, and emoji and ornamental symbols are removed without joining the neighbouring words. Accented and non-Latin letters, punctuation, identifiers and `©`, `®` and `™` are unchanged. Existing listings are presented in normalized form; their public slugs do not change.
- Partner listing writes normalize submitted titles the same way and reject a title without remaining text, such as a blank or emoji-only title, with `BAD_BODY_VALUE`. Synchronous `POST`/`PUT` batches fail with `400`; asynchronous batches report the item as a non-retryable failure. Blank titles were previously stored as empty text.

## 2026-10-19 — Public catalogue overview (#2196)

- New anonymous `GET /api/v1/overview` returns aggregate counts for the public catalogue: `sourcesIndexed`, `liveObjects`, `countries`, `languages` and `newObjectsLast7Days`. Hourly maintenance recalculates them together from PostgreSQL about every six hours; `asOf` is the UTC time of that calculation. It has no filters or personalization and returns no per-source, per-partner or per-user details. Before the first calculation, and while PostgreSQL is unavailable, it returns `503 OVERVIEW_TEMPORARILY_UNAVAILABLE`; an invalid stored calculation returns `500 OVERVIEW_INTERNAL_ERROR`.
- Anonymous `200` responses use `public, max-age=0, s-maxage=900, stale-if-error=0`, so figures are normally at most about seven hours old. Query strings never change the representation; request it without one, since each distinct query string is cached separately. Credentials are not evaluated, but any `Authorization` header makes the response `private, no-store`; request it without a bearer token. Cache lifetimes of existing endpoints are unchanged.

## 2026-10-19 — Strict NONE browsing destination (#2164)

- `PUT /api/v1/me/browsing-destination` rejects a `NONE` destination carrying any member besides `kind` (for example `{"kind":"NONE","country":"FR"}`) with `400 BAD_BODY_VALUE`, as the `BrowsingDestination` schema declares. Previously such a request returned `200`, incremented the version and cleared the existing preference.

## 2026-10-19 — Saved search-filter contract (#2163)

- Saved search-filter response schemas no longer declare `createdBy` or `updatedBy`, matching POST, GET, PATCH and list responses. The POST schema now includes the implemented optional `notifications` flag, which defaults to `true`.
- Every top-level PATCH field accepts `null` as an unchanged value, including `name`, `notifications`, `state`, `search` and `acceptShippingDestination`. Nested search criteria retain their documented clearing rules.
- FREE saved searches reject geography and shipping criteria with `SEARCH_FILTER_RESTRICTED_FEATURE` on creation, addition or activation. Existing affected FREE filters become inactive with their criteria retained. PRO and ULTIMATE allow both predicates in full. Inactive filters can retain or clear existing restricted criteria.

## 2026-10-19 — Personalized watchlist PATCH response (#2162)

- `PATCH /api/v1/me/watchlist/{productListingId}` now returns the documented personalized product details (`item` and `userState`), including for an empty `{}` no-op, instead of a bare watchlist entry. `userState.watchlist` reflects the updated activation and notification settings; hidden/withdrawn redaction is preserved. POST responses are unchanged.
- The route now honors `language` and `currency` (defaults `en` and `EUR`) and rejects unsupported values with `400 BAD_QUERY_PARAMETER_VALUE` before updating the entry. Delegated callers still need only `watchlist:write` for the update and its response.
- OpenAPI no longer permits `WatchlistProductPatch.state: null`, matching the existing `400 BAD_BODY_VALUE` behavior. Omit the field to leave activation unchanged.

## 2026-10-19 — CORS-exposed trace headers (#2159)

- Browser clients can now read the `X-Request-Id` and `X-Correlation-Id` response headers on uncached API routes, as documented. API Gateway previously exposed only `Idempotency-Key` through CORS. The six cache-enabled discovery behaviors still remove both trace headers at CloudFront.

## 2026-10-19 — Cognito account deletion (#2170)

- Self-service and administrator account deletion now durably request Cognito profile removal in the account deletion transaction. Backend credentials remain immediately invalidated; HTTP `204` confirms business deletion while provider removal retries asynchronously.
- The signup address becomes available after provider removal. A new signup creates a new account; deleted identity fences reject delayed registration callbacks. Minimal identity metadata is retained for replay protection and provider retry reconciliation. See [account lifecycle](account-lifecycle.md).

## 2026-10-19 — ARS shipping currency (#2160)

- The OpenAPI `ShippingCurrencyCode` enum now includes `ARS`, matching `CurrencyData`. Shipping profile and import amounts and listing shipping display prices (for example `?currency=ARS`) already accepted or returned `ARS`; clients generated from the old contract rejected those payloads and must be regenerated.
- Saved searches in `ARS` whose shipping maximum requires FX conversion now record matches. Previously, matching such a listing failed instead of creating a match.

## 2026-10-19 — ProductListing search cursor validation (#2158)

- `GET /api/v1/product-listings` rejects malformed inner `searchAfter` sort values or an incorrect sort value count with `400 BAD_QUERY_PARAMETER_VALUE`, identifying `searchAfter` as the query field, instead of returning `503`. Cursors remain opaque and must be passed back unchanged with the same search parameters.
- Continuations retain their original keyword or semantic ranking across embedding-provider recovery or failure. A semantic continuation that cannot obtain an embedding returns `503 PRODUCT_LISTING_TEMPORARILY_UNAVAILABLE` and can be retried with the unchanged cursor.

## 2026-10-19 — Edge request-body limit (#2154)

- JSON request bodies up to the documented 1 MiB limit now reach the API through CloudFront. Bodies above 8 KiB, such as larger shipping profiles or ingestion batches, were previously rejected with an HTML `403` before authentication or validation.
- Requests declaring a `Content-Length` above 1 MiB are rejected at the edge with `413` and the `BAD_BODY_VALUE` error body.

## 2026-10-19 — Synchronous Partner API authorization and conflict documentation (#2169)

- The OpenAPI contract for synchronous batch `POST`, `PATCH`, `PUT` and `DELETE` on `/api/v1/listing-sources/{listingSourceId}/product-listings` no longer claims that Aura Historia administrators are allowed callers. As enforced, each entry requires a user with a Partnership grant for the path ListingSource; administrator role alone returns `403 FORBIDDEN`. Administrators use `/api/v1/admin/product-listings`. An authenticated empty array returns `200 []` without Partnership or scope checks.
- The batch response declarations now include the first-failure statuses the API already returns: `409 CONFLICT` for `POST`, `PATCH` and `PUT`, `404 AUCTION_NOT_FOUND` / `PRODUCT_LISTING_NOT_FOUND` where applicable, and `503 AUCTION_TEMPORARILY_UNAVAILABLE`. Runtime behavior is unchanged.

## 2026-10-19 — Saved search-filter `excludeProductListingId` (#2179)

- Saved search filters name the excluded-listings field `search.excludeProductListingId`, matching the `GET /api/v1/product-listings` query parameter. This applies to `POST /api/v1/me/search-filters` request bodies and all search-filter responses. `search.excludeProductId` is no longer read or returned; existing filters keep their exclusions and now return them under the new name. Invalid values report `search.excludeProductListingId` as the body field. Free-tier allowed fields are unchanged apart from the new name.

## 2026-10-19 — Search `excludeProductListingId` documentation (#2155)

- The OpenAPI contract for `GET /api/v1/product-listings` now names the exclusion query parameter `excludeProductListingId`, which is the name the API has always read. The previously documented `excludeProductId` was never supported on this endpoint and is ignored; clients generated from the old contract must switch to `excludeProductListingId`. Values must be strict `pl_` TypeIDs. Saved search-filter bodies are unchanged.

## 2026-10-19 — ProductListing search default page size (#2157)

- `GET /api/v1/product-listings` without `size` now returns up to the documented default of 21 listings per page, for both keyword and semantic search. Previously it returned 10 (keyword) or 20 (semantic).

## 2026-10-19 — Product search sort validation (#2156)

- `GET /api/v1/product-listings` applies `sort` without `order` using descending order instead of silently falling back to the default relevance ordering.
- Invalid `sort` or `order` values now return `400` (`BAD_SORT_VALUE` / `BAD_ORDER_VALUE`) even when the other parameter is absent. `order` without `sort` returns `400 BAD_SORT_VALUE`.

## 2026-10-19 — Percent-encoded deep-object query keys (#2153)

- All query parameters accept nested `deepObject` keys with percent-encoded brackets (for example `geography%5Bcountries%5D%5B0%5D=DE`, as produced by standard HTTP client encoders) exactly like literal brackets. Listing search geography and shipping filters were previously ignored for such keys, returning unfiltered results, and Party-location discovery rejected them with `400 BAD_QUERY_PARAMETER_VALUE`. Encoded keys now apply the same predicates, validation, pagination cursors and cache behavior.

## 2026-10-19 — Unicode saved-search names (#2161)

- Saved-search `name` limits now count Unicode characters, as documented. Names of up to 255 characters, including multibyte characters, are accepted on create and update instead of returning `500 SEARCH_FILTER_INTERNAL_ERROR`. Longer names are truncated to 255 characters.

## 2026-10-18 — Consistent entity freshness (#2032)

- ProductListing `updated` now advances for every committed change to listing-owned state, including enrichment: title translations, embedding, effective geography, effective shipping summary, content assessment and newly accepted delivery evidence. It is strictly increasing per listing, and each new value has millisecond precision. Enrichment still does not change `expectedVersion` or public history; FX captures and response-time conversion never advance it.
- Saved-search scheduled matching applies the same `updated` interval to every filter. Shipping filters no longer revisit older listings on every run: an older listing becomes eligible after an actual persisted change (for example a shipping profile/default or geography change), and FX-only threshold crossings no longer create matches for listings outside the interval.
- Partnership `updated` advances when memberships or listing-source grants change. Repeating an identical notification `seen` state succeeds without changing the notification's `updated`.

## 2026-10-17 — Token revocation enforcement

- Cognito session revocations requested through the administrator API are enforced by a persisted cutoff in the existing identity lookup, without a live Cognito status request. Signed `auth_time` and `iat` are required; refresh cannot bypass the cutoff. A 60-second clock-skew allowance can temporarily reject fresh logins. Provider errors can leave local revocation applied. Revocations made only in Cognito remain unobserved until JWT expiry.
- OAuth token revocation rejects tokens issued to another client and personal access tokens with `403 FORBIDDEN`, preserving them. Unknown and already-revoked tokens still return `200`.

## 2026-10-17 — Advertised shipping and collection (#1997)

- Listing detail, search, similar, watchlist, saved-search match and auction catalogue responses include `item.shipping`: current advertised terms, observation/dependency information and converted rates in the requested display currency. These are advertised arrangements, not checkout quotes or landed-cost totals.
- Users with an authorized selected browsing destination also receive `userState.shipping`, including uncertainty when purchase inputs are missing. Delegated personalization requires `users:read`. Hidden/withdrawn entries expose no shipping; sites and private address identities are omitted. Authenticated responses/errors are `no-store`.
- Search and saved filters support whole-country shipping, optional service, per-item maximum and independent collection-offered predicates. Maximums compare source currency by default; explicit `WHOLE_COUNTRY_PER_ITEM_FX_V1` uses the declared target currency and pinned FX snapshot. Indexed membership/counts may lag current response terms.
- Saved shipping destinations are fixed private country snapshots; address/preference edits do not move them. Optional `acceptShippingDestination` checks an explicitly authored country against an owned address or selected preference. FX-mode matches expose their immutable shipping valuation basis.
- Added source-scoped shipping profile list/detail, idempotent creation, complete replacement, retirement/restoration and default assign/clear APIs for administrators and authorized source operators. Commands require current management/default revisions; missing/retired references do not fall back to another profile. Delegated credentials require `listing-sources:read/write`.
- Added protected source-keyed listing delivery inspection, revision-checked PATCH and asynchronous submission for selection, rate inputs and independent shipping/collection overrides. Delegated credentials require `product-listings:read/write`. Public listing history adds address-free `DELIVERY_CHANGED`.
- Management commands accept optional incremental import identity, ordering and acquisition metadata. Stable object keys support recovery, exact retries retain identity, and manual components are protected. Provider scope is source plus enabled method. Terms continue until changed, withdrawn or authority is revoked; observation age alone never expires them.

See [shipping](shipping.md) and [OpenAPI](swagger.yaml) for request shapes, concurrency and failure behavior.
## 2026-10-15 — Argentine peso support (#2095)

- Currency fields now accept `ARS` (Argentine peso), including user preferences, ListingSource configuration and fallback currency, listing prices, and search filters. Amounts use two ISO 4217 minor-unit decimal places.

## 2026-10-14 — Keyless address and Party-location creation (#2078)

- `POST /api/v1/me/addresses` and `POST /api/v1/parties/{party_id}/locations` accept creation without `Idempotency-Key`. Legacy headers are ignored, including invalid/repeated values, and are not echoed. Each accepted ordinary create assigns a new ID, even for equal content; keys provide no deduplication or result recovery.
- Automatic creation retries can produce duplicates. After an ambiguous timeout, refresh current state and reconcile before creating again; address/label equality cannot conclusively identify the original request.
- Relocation atomically retires the old location and creates its replacement using the old expected revision. Repeating a committed relocation with that revision returns `409` without another replacement.

## 2026-10-13 — Saved-search matching across tiers (#2040)

- Active ordinary and enhanced saved searches discover matches in scheduled runs. Each run evaluates the first up to 50 eligible candidates independently; rejected candidates do not stop discovery. Matching remains bounded and subject to projection lag.
- Free uses listing title/description, Pro adds up to one image, and Ultimate adds up to four images and enhanced instructions when supplied. Filters with only structured constraints require no semantic evaluation. Plan reconciliation preserves authored instructions and fences obsolete matching results when permitted evidence changes.

## 2026-10-13 — Partner ProductListing management (#2039)

- Added source-scoped management directory/detail, versioned single-listing PATCH/DELETE and explicit POST restore. Path parameters use `listingSourceId` and `productListingId`. PostgreSQL reads include both ACTIVE and WITHDRAWN listings and expose `expectedVersion`; summaries use bounded filters and source-bound keyset cursors. Responses and errors use `private, no-store`.
- Every route requires an active Partnership grant for the path ListingSource. Delegated reads require `product-listings:read`, mutations require `product-listings:write`; administrator role alone grants no access. Listings outside the authorized source return the same `404 PRODUCT_LISTING_NOT_FOUND` as missing listings.
- Mutations share admin field validation, optimistic concurrency, lifecycle and atomic event behavior. Withdrawal retains history and relationships; restore never creates a listing or recreates availability. Batch/async ingestion, PUT upsert/restore, public discovery and administrator authorization keep their existing behavior.

## 2026-10-13 — Party location manager directory (#2044)

- Added administrator-only `GET /api/v1/admin/parties/{party_id}/location-managers`, with optional exact `userId` and `access=EXPLICIT|PARTNERSHIP` filters. Each user appears once with email, optional names and all independent current access reasons; Partnership reasons include active `partnershipIds`. Grant timestamps are not stored or returned. Revoking an explicit grant preserves Partnership access; suspended/deleted users and dissolved Partnerships are excluded.
- Delegated credentials require `parties:read`. Results use ascending user UUID keyset pagination with `size` clamped to 1–100 (default 21); terminal pages omit `searchAfter`. Responses and errors are `no-store`; unknown Parties return `404 PARTY_NOT_FOUND`.

## 2026-10-13 — ProductListing administration (#2038)

- Added admin ProductListing directory/detail, PATCH, retained DELETE withdrawal and explicit POST restore. Reads include active/withdrawn originals and privileged canonical image URLs. Responses and errors use `private, no-store`; public discovery and image filtering are unchanged.
- Delegated admin reads require the new `product-listings:read` scope; writes require `product-listings:write`. Persisted administrator authority is always required for HTTP users, without a Partnership prerequisite. The forward scope migration accepts the new scope without broadening existing grants.
- Mutations require `expectedVersion`, preserve immutable identity/text and use the ordinary atomic state/event pipeline. Stale/racing writes return `409 PRODUCT_LISTING_CONCURRENCY_CONFLICT`; PATCH of withdrawn listings returns `409 PRODUCT_LISTING_WITHDRAWN`. No-ops preserve versions and timestamps. Later ingestion can overwrite edits or restore withdrawn listings; downstream effects converge asynchronously.

## 2026-10-12 — Periodic saved-search matching (#2036)

- New saved-search matches are discovered by scheduled matching of active enhanced filters only. Non-enhanced filters no longer discover new matches. Notifications for newly discovered matches wait for a scheduled run rather than listing-change processing. Saved-filter CRUD, historical match records, feedback and notification settings are unchanged; request and response shapes are unchanged.

## 2026-10-12 — Geographic discovery and private addresses

- Listing search and saved filters accept one `geography` predicate: ISO countries/subdivisions, an explicit numeric origin, a radius with units, strict/approximate precision and optional distance order. Listing summaries expose one source-derived geography; backend inventory applicability determines eligibility. Strict proximity requires premises evidence. Geographic responses are `private, no-store` and accept projection lag. Saved origins are private snapshots independent of address edits/preferences.
- Added anonymous `GET /api/v1/party-locations` with bounded keyset discovery. Active locations permit exact eligible geography; public views exclude raw addresses, management evidence and revision tokens.
- Added protected PartyLocation management under `/api/v1/parties/{party_id}/locations`, supporting multiple sites, idempotent creation, same-site correction, relocation and retirement/restoration. Mutations require current revision. Administrators, active partnership members or explicit location managers may operate; delegated callers also require the relevant Party scope. Admin grant/revoke routes manage independent location grants.
- Added protected and admin ListingSource `location-default` GET/PUT/PATCH/DELETE routes. Each source selects at most one operator-owned location with explicit business/inventory applicability evidence. Current revision, active-site eligibility and source authority are required; clear remains possible after retirement. Responses are `no-store`.
- Added private `/api/v1/me/addresses` create/list/get/replace/delete routes with server-generated `uaddr_` IDs, owner-scoped `Idempotency-Key` creation and CAS replacement/deletion. Ordinary reads expose resolution status, permitted derived geography and attribution. Unique compatible premises results may resolve automatically; uncertain inputs remain saved and editable.
- Added GET/PUT `/api/v1/me/browsing-destination` with CAS choices `NONE`, `ADDRESS` or `COUNTRY_POSTAL`. Deleting the selected address clears the preference. Address/preference routes require a current unsuspended owner and corresponding delegated user scope; all responses are `no-store`. They do not implicitly supply listing-search origins.

## 2026-10-10 — Historical ProductListing price display (#2018)

- `GET /api/v1/product-listings/{productListingId}/history` accepts optional `currency`. Omission preserves the existing response. Monetary discovery prices/estimates and both sides of price/estimate changes can include additive `display` values with the requested currency, minor-unit amount, `fxRateId` and `capturedAt`.
- Each entry uses the latest persisted FX snapshot at or before its timestamp, with HalfUp rounding (JPY/KRW: 0 decimals; others: 2). Source values remain unchanged. Missing snapshots or required historical quotes omit that value's `display` while still returning `200`; consumers fall back to labelled source values. `ON_REQUEST` and null values have no projection. Current/later rates are never substituted.
- Unsupported currency returns `400 BAD_QUERY_PARAMETER_VALUE`. Anonymous success keeps its 300-second shared cache, with currency variants in the edge cache key; credential-bearing responses and errors stay `private, no-store`.

## 2026-10-09 — Additional supported currencies (#1578)

- Currency fields now accept `SEK`, `DKK`, `NOK`, `KRW`, `INR`, `TWD`, `HUF`, `RON`, `MXN`, and `THB`, including user preferences, listing-source configuration, listing prices, and search filters. `KRW` amounts use zero decimal places; the other additions use two ISO 4217 minor-unit decimal places.

## 2026-10-09 — Delegated credential scope coverage (#1993)

- Admin Auction create/update require `auctions:write`; Party reads/writes require `parties:read`/`parties:write`; private ListingSource reads and admin writes require `listing-sources:read`/`listing-sources:write`; the admin overview requires `admin-overview:read`.
- PartnershipApplication reads (own and admin) require `partnership-applications:read`; submission, withdrawal, review and decisions require `partnership-applications:write`. Admin Partnership reads and mutations require `partnerships:read` and `partnerships:write` respectively.
- Private notification reads require `notifications:read`; seen-state updates and deletion require `notifications:write`. SearchFilter and match reads now require `search-filters:read`; writes still require `search-filters:write`. A write scope does not imply read access.
- These scopes are accepted by access-token and OAuth flows. Missing delegated scopes return `403 FORBIDDEN` before protected work. Admin roles and ownership checks still apply. Cognito callers keep their existing authorization policy. Existing credentials and OAuth grants are not broadened automatically; explicitly grant the required scopes. Apply the credential registry migration before issuing credentials with new scopes.

## 2026-10-09 — Auction admin search and public schedule ordering (#1991)

- Added canonical access-token/OAuth scope `auctions:read`. Both admin Auction GET endpoints now require it for delegated tokens, in addition to administrator authority; missing capability returns `403 FORBIDDEN`. Cognito authorization, admin create/update, and public reads are unchanged. Existing token and OAuth grants are not broadened automatically; grant the scope explicitly where needed.
- Added admin-only `GET /api/v1/admin/auctions` with `query`, `listingSourceId`, `sourceAuctionId`, `format`, `reportedStatus`, `sort`, `order`, `size`, and opaque `searchAfter`. It returns complete `AuctionAdminData` rows regardless of status or listing visibility; `listingSourceId` plus `sourceAuctionId` resolves create conflicts. NUL-containing search text and cursor name keys are rejected with `400 BAD_QUERY_PARAMETER_VALUE`. Results and errors are `no-store`.
- Public `GET /api/v1/auctions` now accepts `sort=created|scheduled` and `order=asc|desc`. The default remains newest-created-first and preserves legacy cursors; scheduled sorting requires `timeRole`, defaults to ascending, sorts missing instants last without a window, and scopes its cursor to sort/order. Anonymous cache policy remains unchanged.

## 2026-10-09 — Stable ProductListing title reads

- ProductListing titles preserve truncation ellipses across normalization and persistence. Removing sentence-final punctuation also removes exposed outer whitespace.
- Details and history can read the bounded trailing-whitespace titles emitted by the earlier constructor. History payloads, public IDs and slugs are preserved; response shapes are unchanged.

## 2026-10-09 — Consistent profile-name normalization (#1984)

- Profile-name JSON input now uses the existing `FirstName`/`LastName` constructors. Names are consistently truncated to 64 Unicode characters before persistence; the documented maximum and wire shape are unchanged.

## 2026-10-08 — Newsletter confirmation template failures

- Temporary failures fetching a confirmation email template now return `503 NEWSLETTER_TEMPORARILY_UNAVAILABLE`. Missing or invalid templates remain `500 NEWSLETTER_INTERNAL_ERROR`; no SES send occurs when template preparation fails.

## 2026-10-07 — Newsletter double opt-in API (#1945)

- `PUT /api/v1/newsletter-subscriptions` always requests double opt-in, including for an authenticated user's exact account email. Optional bearer authentication supplies profile fallback only; invalid supplied credentials return 401. New and rate-suppressed requests return empty `204` with `Cache-Control: no-store`, without revealing account or pending state. The endpoint no longer writes directly to Loops.
- Added anonymous `POST /api/v1/newsletter-subscriptions/confirm` with exactly `{"token":"..."}`. It returns empty `204` after the confirmation, consent-state, and asynchronous Loops intent transaction commits; retained confirmed-token replay is idempotent. GET does not confirm consent, and success does not mean Loops accepted the later update.
- These small JSON bodies are limited to 8 KiB and confirmation tokens to 512 bytes; oversized requests, including those rejected at the shared transport limit, return `400 BAD_BODY_VALUE`. Invalid proof uses one generic `400 NEWSLETTER_CONFIRMATION_INVALID`; temporary persistence failures, SES throttling/limits, and ambiguous SES acceptance return `503 NEWSLETTER_TEMPORARILY_UNAVAILABLE`; permanent definite SES rejection and other internal failures return `500 NEWSLETTER_INTERNAL_ERROR`. Results are `no-store` and errors omit email, token, and provider details.

## 2026-10-07 — Signed Loops preference webhook (#1948)

- Added anonymous `POST /api/v1/webhooks/loops`; it requires valid `webhook-id`, `webhook-timestamp`, and `webhook-signature` proof over the exact request bytes, with no Aura bearer. The body limit is 64 KiB and the processing budget is 10 seconds.
- `204` acknowledges only a committed C13 receipt/application, identical duplicate, or safely ignored verified event. Invalid proof returns `401 LOOPS_WEBHOOK_UNAUTHORIZED`; malformed selected body/event returns `400 LOOPS_WEBHOOK_INVALID_BODY`; delivery identity conflict returns `409 LOOPS_WEBHOOK_CONFLICT`; oversized body returns `413 LOOPS_WEBHOOK_PAYLOAD_TOO_LARGE`; internal configuration failure returns `500 LOOPS_WEBHOOK_INTERNAL_ERROR`; and retryable provider/database/deadline failures return `503 LOOPS_WEBHOOK_TEMPORARILY_UNAVAILABLE`. Responses are `no-store`.

## 2026-10-07 — Own access-token request and response DTOs (#1965)

- `POST /api/v1/me/access-tokens` requires `name` and `scopes` (including `[]`), accepts optional `expires`, and returns only `userId`, `accessTokenId`, and the one-time plaintext `accessToken`. The create response is `Cache-Control: no-store`.
- Own list/get/update responses use `userId`, `accessTokenId`, `name`, `scopes`, `origin`, and optional `expires`. They never return plaintext or masked token values.

## 2026-10-05 — Remove saved-search match explanations (#1847)

- Removed `matchReason` from ProductListing user-state responses and `enhancedMatchReason` from saved-search match records. Match feedback is unchanged; classifier scores and generated explanations are not persisted or returned.

## 2026-10-05 — Read-only current email marketing consent (#1935)

- Added `marketingEmailConsent` to authenticated own-account and admin user detail reads. The field reports the current email-marketing grant and cannot be changed through account PATCH.

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
