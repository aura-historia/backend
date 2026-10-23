# MCP transport

The canonical MCP endpoint is `https://mcp.aura-historia.com/v1`. The
`aura-historia-mcp` crate serves the same `/v1` router natively and through
Lambda HTTP API v2. The [front-door contract](http-api-front-door.md#mcp-front-door)
owns hosting, certificates, Origin/CORS and stage verification. Native and Lambda entry
points share one registry and authenticate each request independently. Configure a generic
remote MCP client with this URL and Streamable HTTP transport. Public catalogue discovery and
public calls require no account; protected operations use opaque Aura access tokens.
The shared registry contains 38 operations: 12 public, 9 account, 7 saved-search, 4 watchlist
and 6 notification operations. Ten public operations forward validated delegated context.

## Protocol compatibility

The official Rust SDK is pinned to `rmcp = 3.6.0`. The supported matrix is
exercised at the HTTP boundary and with official SDK clients in
[`transport.rs`](../src/aura-historia-mcp/tests/transport.rs).

| Revision | Entry flow | Subsequent requests |
| --- | --- | --- |
| `2026-07-28` | `server/discover` or an ordinary request | Self-contained protocol version, client identity and capabilities in `params._meta`; matching `MCP-Protocol-Version`, `Mcp-Method` and applicable `Mcp-Name` headers |
| `2025-11-25`, `2025-06-18`, `2025-03-26` | `initialize`, then `notifications/initialized` | Stateless POST; version header recommended, absent header uses `2025-03-26` |

Each POST contains one UTF-8 JSON-RPC message, uses `Content-Type:
application/json`, and accepts both `application/json` and
`text/event-stream` as required by Streamable HTTP. Responses are bounded
JSON; accepted notifications receive an empty `202`. The transport advertises
tools without streaming, subscriptions, sampling, elicitation or tasks.
HTTP GET and DELETE at `/v1` return `405` with `Allow: POST`. The old
HTTP+SSE transport is excluded.

Legacy initialization negotiates a supported legacy revision through the SDK.
No flow creates or requires a session ID; a supplied legacy session ID has no
authorization meaning and is never echoed. Each request can reach a different
Lambda invocation. Supplied routing headers must agree with body identity even
on legacy calls. Duplicate routing or credential headers are rejected; encoded
`Mcp-Name` values are decoded before comparison. The SDK additionally validates
current per-request metadata and any schema-declared parameter headers.

The version allowlist rejects unsupported non-initialize revisions with HTTP
`400`, JSON-RPC `-32022` and the supported revisions. Unknown methods return
`-32601` (HTTP `404` for current requests, `200` for legacy requests). Unknown
tools, malformed arguments and invalid catalogue cursors return `-32602`.
Malformed JSON receives `400`/`-32700`. An invalid JSON-RPC version, method type
or request ID receives `400`/`-32600` with a null response ID. Identity-header
disagreement receives `400`/`-32020`. Wrong content type and unacceptable response
types receive `415` and `406` respectively.

The normative contracts are the [current Streamable HTTP specification](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/streamable-http),
[current versioning rules](https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning)
and [legacy Streamable HTTP specification](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports).
SDK upgrades must rerun the matrix before expanding the supported revisions.

## Operation families and admission

Families register `OperationDefinition::new<I, O>` with the selected existing
API operation ID, a description, access class, required Aura scopes, and all
four mutation/retry/open-world annotations. The
[public operation manifest](../src/aura-historia-mcp/public-operations.json) and
[authenticated operation manifest](../src/aura-historia-mcp/authenticated-operations.json)
record disjoint subsets of the same endpoint and registry. Together they record the selected
allowlist, family mappings, access policy, annotations and exclusions. `tools/list`
publishes each callable operation's generated `inputSchema` and `outputSchema`. A closure
captures narrow service handles and calls the shared
inbound application use case directly. Use shared `aura-historia-contracts`
DTOs and constructors; transport wrappers may assemble path/query/body inputs
but must not duplicate business fields or business behavior.

Input schemas use Schemars deserialization direction; output schemas use
serialization direction. Unsigned integer schemas include the exact bounds of their wire
types. Inputs must be objects, including tagged unions whose
variants are all objects. Object results retain their shared DTO shape. Array,
scalar, nullable or otherwise non-object root schemas use
`{ "result": <shared response> }`, because legacy MCP output schemas
require an object. The same value is emitted as structured content and JSON
text for older generic clients. Neither representation drops fields. A result
that exceeds the limit fails explicitly; collection pagination belongs in the
shared use case and must never become transport truncation.

The registry rejects duplicate IDs, lists tools lexically by operation ID,
and returns at most 50 definitions with an explicit continuation cursor. Its
manifest includes `aura.historia/operationId`, `aura.historia/access` and
`aura.historia/requiredScopes` metadata. Protected tools remain discoverable.
Health and readiness are HTTP-only `/health` and `/ready` probes, outside the
operation registry.

`AdmissionGate` runs before SDK dispatch and tool-result wrapping, after body and routing
headers agree. The production `AuraTokenGate` invokes `AuthenticateAccessTokenUseCase`
directly for every supplied credential, then builds a fresh `DelegatedUser` with exactly the
stored capabilities. REST and MCP reuse the narrow `aura-historia-auth` adapter; Cognito JWTs
and unrelated bearer credentials are rejected at MCP. The configured environment's Aura token
store and shared resource policy recognize REST and MCP as one logical resource, including
legacy personal/OAuth tokens. No identity or authorization is cached in an MCP session.

Absent credentials permit discovery and public operations. Supplied malformed, expired,
revoked or invalid credentials receive HTTP `401` with `error="invalid_token"`, including on
public calls. Protected calls without credentials receive HTTP `401`; insufficient grants
receive HTTP `403` with `error="insufficient_scope"`. Their Bearer challenges include
`resource_metadata` and every scope required by the selected operation, rather than only the
missing subset. Authentication-store outages receive bounded HTTP `503` with `Retry-After: 1`;
invalid persisted/internal authentication state receives `500`. Neither becomes anonymous
success or an invalid-token challenge. Rejected requests never execute a tool. Use cases
remain the authority for scope, role, partnership and ownership checks.

Both `/.well-known/oauth-protected-resource` and
`/.well-known/oauth-protected-resource/v1` serve credential-free RFC 9728 metadata. They
identify the exact canonical MCP resource and configured Aura issuer, advertise only header
bearer credentials and the baseline `users:read` scope. Operation challenges provide the
required scope set for each protected call. Host/Origin checks and `Cache-Control: no-store`
apply to both routes. Challenges point to the path-suffixed metadata URL. Aura authorization
server discovery belongs to [the OAuth contract](oauth.md).

Family adapters classify expected input, missing-resource, conflict, forbidden
and temporary failures into distinct static tool-error codes. Invalid arguments
are protocol errors; unexpected failures are safe `-32603` errors. Technical
causes remain attached to internal error types and never appear in client
messages. No mutation is automatically retried: a timeout or oversized response
may follow a completed write, so callers must follow the operation's existing
idempotency and recovery contract.

## Watchlist tools

| Operation | Scope | Result |
| --- | --- | --- |
| `getWatchlistProductListings` | `watchlist:read` | Complete personalized listing page with returned item count and optional total/cursor |
| `addWatchlistProduct` | `watchlist:write` | Created watchlist entry, including notification setting and state |
| `deleteWatchlistProduct` | `watchlist:write` | Bodyless REST outcome represented as `{ "result": null }` |
| `patchWatchlistProduct` | `watchlist:write` | Complete personalized listing after notification/state changes |

Every request targets the validated delegated user's own watchlist; no user identifier argument
is accepted. Reads retain language/currency defaults and size clamping. Serialize the returned
`searchAfter` timestamp/listing-ID array as a JSON string for the next request. Add notifications
default to `true` when omitted or null. Patch omission preserves a field; explicit null is invalid
for notification/state fields, and only `ACTIVE` or `INACTIVE_BY_USER` can be requested. Update
also accepts the shared language/currency presentation options. Service-owned hidden-content,
image, tier and scoped `userState` disclosure rules apply to complete listing results.

Duplicate adds return `CONFLICT`; missing removes/updates return `NOT_FOUND`; activation/add
quota failures return `WATCHLIST_QUOTA_EXCEEDED`. Mutations are annotated idempotent because duplicate
adds and missing removes make no further state change, and patch sets absolute values. Repeated
calls still retain their conflict/missing outcomes; deactivation/removal are destructive. These hints
do not grant authority or authorize retries. A mutation timeout leaves the outcome unknown;
inspect current state before deciding whether to retry. REST and MCP reuse shared watchlist
contracts and the same typed runtime service family. The
[operation fixtures](../src/aura-historia-mcp/src/tools/watchlist/tests.rs) and
[local database acceptance](../src/aura-historia-mcp/tests/watchlist_postgres.rs) cover the boundary.

## Public catalogue

The public subset remains callable without credentials. Its ten listing, history, auction and
source operations forward a supplied, validated `DelegatedUser` directly to the existing service
use cases, preserving the same tool names, full contracts, filters and continuations. Location
discovery and public overview remain anonymous. Public operations require no scopes and advertise
read-only, idempotent, nondestructive behavior against Aura's catalogue. Services remain the
authorization authority. The public manifest describes this subset of the hybrid endpoint;
protected families have separate, non-overlapping registrations. Newsletter opt-in/confirmation, health/readiness,
webhooks/simulation and OAuth protocol/control-plane endpoints are explicitly excluded.

| Tool / source operation ID | Arguments and result |
| --- | --- |
| `simpleSearchProductListings` | Complete existing filters, language/currency, `sort`, `order`, `size`, `searchAfter`; returns the cursored personalized summary collection and service-supplied totals. |
| `getProductListing` | `productListingId`, language/currency; returns the complete public detail envelope. |
| `getProductListingByTitleSlug` | `productListingTitleSlugId`, language/currency; returns the same detail envelope. |
| `getSimilarProductListings` | `productListingId`, language/currency; returns `EMBEDDING_PENDING` or `READY` with `items`. |
| `getProductListingHistory` | `productListingId`, optional `currency`; returns every shared history entry inside `{ "result": [...] }`. |
| `listAuctions` | Source, format, reported status, schedule range, `timeRole`, sort/order, `pageSize`, `searchAfter`; returns the public auction directory. |
| `getAuction` | `auctionId`; returns source metadata, schedule, reported status/counts, visible count and catalogue/outbound links. |
| `getAuctionCatalogue` | `auctionId`, language/currency, `pageSize`, `searchAfter`; returns public lots with existing listing presentation. |
| `searchPublicListingSources` | `query`, `size` (default 21, 1–50), `searchAfter`; absent/blank text browses and insufficient text returns no matches. |
| `getPublicListingSourceBySlug` | `listingSourceSlugId`; returns the public source card, operator name and optional URL/image. |
| `discoverPartyLocations` | Complete `geography`, `size` (default 21, 1–100), `searchAfter`; returns public location IDs, basis, geographic context, point, precision and distance. |
| `getPublicOverview` | Empty object; returns `asOf` and every stored public catalogue aggregate. An uncalculated overview is temporarily unavailable. |

Search retains geography, shipping, all identifier inclusion/exclusion sets, price/date ranges,
availability, orderability and unspecified-availability predicates. Geography includes countries,
subdivisions, numeric origin, radius/unit, mode and distance order. Shared contracts preserve
presentation defaults, size clamping, localization/fallback, source-local identifiers, full image
collections and long URLs, content policy, shipping disclosure and withdrawn-listing handling.
Anonymous results omit `userState`. Delegated listing results disclose `watchlist` only with
`watchlist:read`, `searchFilter` (including saved-search references and match feedback) only with
`search-filters:read`, `notification` only with `notifications:read`, and `contentVisibility` and
stored destination-derived `shipping` only with `users:read`. Unauthorized sections are omitted;
no applicable sections means `userState` itself is omitted. Write scopes do not imply read scopes.
Stored hidden-item, tier and sensitive-image policy still applies with every scope combination,
including an empty grant. No stored browsing destination is read without `users:read`.
Public source/location/auction output excludes management, contact, grant and provider configuration. Unsigned amounts and opaque integer sort values remain
exact JSON integers.

Similarity uses the service's fixed neighbor limit. `EMBEDDING_PENDING` means retry the same
operation later; `READY` with an empty array is a completed result. The envelope represents REST's
bodyless pending outcome and introduces no neighbor-count argument.

### Continuations and complete history

| Operation | Continuation contract |
| --- | --- |
| Listing search | Supply the returned `searchAfter` object directly as JSON. Its immutable FX snapshot and complete opaque sort values are retained. REST alone query-encodes that object. |
| Auction directory/catalogue | Serialize the returned `searchAfter` object as the next argument's JSON string, as on REST. Directory scope includes all filters/sort and nullable schedule instants; catalogue scope includes auction, position and listing identity. |
| Source search | Return the opaque cursor unchanged with the same canonical query and page size. Encoded/decoded cursor bounds remain 4 KiB/3 KiB. |
| Location discovery | Supply the returned object containing `locationId` and optional `distanceKm`; preserve the geographic scope. |
| History and similarity | No paging or continuation arguments. |

Auction scheduled sorting requires `timeRole`; a schedule range requires both bounds. Catalogue
pricing retains the use case's current/sale-observation FX selection. Source search and slug
detail share a non-queuing budget of four active reads with a 500 ms source-read timeout, also
bounded by the invocation deadline. Location validation uses the existing constructors/service;
no address or profile origin is inferred.

History deliberately preserves the full REST array and the existing webapp timeline contract.
Every event retains its original order, including distinct events sharing a timestamp, discovery
and change payloads, IDs, timestamps, source prices and optional historical display prices.
Omitted/null currency leaves source prices unchanged. Each priced event uses its own historical
FX snapshot and provenance; missing historical snapshots or quotes omit the display projection
without substituting a later/current rate. Existing empty histories succeed; unknown listings
return `NOT_FOUND`. There is no event cap, clipped tail or invented cursor. An oversized complete
history fails explicitly with no partial result.

The [combined public acceptance suite](../src/aura-historia-mcp/src/public/tests/mod.rs) uses
fake inbound services and official SDK clients through the same registry as production. Family
contract/operation suites retain detailed input, mapping, validation and error parity coverage.
The [personalization suite](../src/aura-historia-mcp/src/public/tests/personalization/mod.rs)
exercises real listing and shipping handlers through HTTP, including all read-scope combinations,
private-section omission and protective policy. The public authentication suite checks all ten
context paths, including operations with no personalized output.
Local tests exercise both discovery and initialize lifecycles, all twelve tools, complete search
continuation, long image URLs, 128-event history and explicit oversize failures. These tests are
local verification; they do not establish deployment, DNS, WAF behavior or a client-specific bound.

## Own account, addresses and browsing destination

The account family acts only for the user in the validated delegated context. Read operations
require `users:read`; mutations require `users:write`. Services enforce these capabilities and
current ownership independently of transport admission. No argument selects another account.

| Tool / source operation ID | Shared contract |
| --- | --- |
| `getUserAccount` | Complete own-account response, including profile, language/currency/unit, content preference, consent, tier and role. |
| `updateUserAccount` | First/last name, language, currency, measurement unit and sensitive-content display preference; returns the complete own account. |
| `createUserAddress` | Label and complete geography; returns server-generated identity, input revision, version, preferred status and resolution. |
| `listUserAddresses` | `after` and `limit` (default 25, 1–100); returns every item on the page and `next`, including explicit null on the last page. |
| `getUserAddress` | `addressId`; returns the complete owned-address view. |
| `updateUserAddress` | `addressId`, `expectedVersion`, label and complete geography; replaces editable content while retaining identity and owner. |
| `deleteUserAddress` | `addressId` and `expectedVersion`; deletes only that address and returns `{ "result": null }`. |
| `getBrowsingDestination` | Complete version and selected `NONE`, `ADDRESS` or `COUNTRY_POSTAL` choice. |
| `setBrowsingDestination` | `expectedVersion` and the complete destination choice; returns the resulting version and destination. |

Profile patches retain omission as unchanged, null as clearing an optional member, and a value
as replacement. `showUnassessedOrSensitiveContent` accepts a boolean and rejects null. Existing
locale aliases normalize to canonical language codes. Profile fields outside the editable
contract have no effect, as on REST. Address and destination objects reject unknown members.
Geography retains its reference release, address text, country, subdivision and postal code and
uses the same validated constructors as REST. Address bookmarks require non-null `addressText`.
Address resolution includes status, eligible derived geography and point precision/accuracy,
and provider attribution URL.

Pass the address page's `next` unchanged as `after` to continue; no transport cap truncates a
page. Versioned writes preserve stale-version conflicts. `NONE` clears the destination;
`ADDRESS` requires an owned entry; `COUNTRY_POSTAL` accepts an optional postal code. Deleting
the selected address clears the preference atomically. The [private geography contract](geography.md#private-user-address-books-and-browsing-destinations-2005)
owns resolution, privacy and erasure rules.

Address creation assigns a new identity on every accepted call, including equal content.
Its annotations mark it non-idempotent; an ambiguous timeout requires reconciliation before a
retry, as described in [command retries](command-retries.md). Replacements/deletion advertise
destructive effects and preserve service CAS behavior. The transport does not retry mutations.
Credential management, billing and account deletion have no tools in this family.

## Saved searches and match feedback

All seven saved-search operations use the trusted delegated user's identity. Read and write
capabilities remain independent; read operations require their read capability even when a
write grant is present.
Services enforce ownership, tier quotas, restricted criteria and shipping-destination acceptance.
The family shares its [contracts and mappings](../src/aura-historia-contracts/src/saved_searches/mod.rs)
with REST and calls the same inbound services directly.

| Tool / source operation ID | Scope and contract |
| --- | --- |
| `getUserSearchFilters` | `search-filters:read`; empty arguments return every own filter with `items`, `from`, `size`, `total`. No truncation or invented continuation. |
| `createUserSearchFilter` | `search-filters:write`; complete REST body, including `acceptShippingDestination`, nested `search` and optional `notifications` (default `true`). Returns the saved filter. Non-idempotent. |
| `getUserSearchFilter` | `search-filters:read`; `userSearchFilterId` returns the complete own filter. |
| `deleteUserSearchFilter` | `search-filters:write`; `userSearchFilterId` deletes the own filter and returns `{ "result": null }`. Missing filters remain `NOT_FOUND`. |
| `updateUserSearchFilter` | `search-filters:write`; `userSearchFilterId` plus the complete shared REST patch. Returns the saved filter. |
| `listSearchFilterMatches` | `search-filters:read`; `userSearchFilterId`, `language`, `currency`, `size` (default 21, clamped to 1–100) and JSON `searchAfter`. Returns the existing personalized listing details page in ascending match order. |
| `updateSearchFilterMatchFeedback` | `search-filters:write`; `userSearchFilterId`, `productListingId` and optional `feedback`. Returns the stored match snapshot, including its existing shipping valuation. Missing or stale matches remain `NOT_FOUND`. |

Search criteria retain every geography, shipping, identifier inclusion/exclusion, price, date,
text, enhanced-description, availability and orderability predicate. Omitted top-level patch
members and top-level nulls preserve values. Nested patches retain the existing field-specific
null rules: clearable values clear, required values reject null. `availability`, `orderability`
and `includeUnspecifiedAvailability` must be supplied together as three concrete values, or all
null to clear the availability query. Feedback omission preserves its value, null clears it,
and both booleans remain meaningful. Optional address/preference acceptance uses the existing
service's ownership, capability and destination checks; MCP does not infer a destination.

Match continuation is the exact JSON array `[RFC3339 timestamp, ProductListing ID]` from the
response; supply that array directly on the next call. REST alone query-encodes it. Returned
`size` counts emitted items. Listing prices, shipping valuation snapshots, canonical IDs and
scope-dependent `userState` disclosure use the shared listing presentation without extra
hydration or field dropping. Tier failures retain `SEARCH_FILTER_QUOTA_EXCEEDED` and
`SEARCH_FILTER_RESTRICTED_FEATURE`; other failures use the common safe error classes.

Creation and updates persist saved criteria; they do not complete background matching. Match
listing reads materialized results from scheduled matching and remains subject to projection
lag. An empty page is a current result, not evidence that matching has finished. No mutation is
automatically retried. A timeout, unavailable response or response-size failure can follow a
committed write; creation with an unknown outcome must be reconciled by listing own filters
before deciding whether to submit another create. The adapters do not claim exactly-once execution.

## Notification inbox

The six notification operations share REST contracts and service use cases. `listNotifications`
requires `notifications:read`; mutations require `notifications:write`. Every call operates on
the delegated user's inbox. Stored listing snapshots, all notification payload kinds, localized
titles and prices are preserved. The service reads the user's sensitive-content preference
internally; listing notifications requires no additional user, saved-search or watchlist scope.

Listing accepts `language`, `size` (default 21, clamped to 1–100) and `searchAfter`. Serialize the
returned `[created, notificationId]` cursor array as a JSON string for continuation. `size` counts
returned items; an exhausted page omits `searchAfter`, and no total is supplied.

`updateNotificationSeen` sets one owned item's `seen`. `updateNotificationsSeen` sets selected
`notificationIds`, deduplicates them, ignores missing/foreign IDs and rejects an empty selection.
`updateAllNotificationsSeen` applies the supplied `seen` boolean to the entire inbox, including
unlisted pages. `deleteNotification` permanently deletes one `notificationId` and returns not
found for missing/foreign items. `deleteNotifications` accepts only `{}` and permanently deletes
the entire own inbox; it has no ID selection or request body. Successful mutations acknowledge
with `{ "result": null }`, preserving REST's bodyless business outcome without counts or per-item
reports. Mutations are not automatically retried; after an ambiguous timeout the outcome is unknown.

## Party locations and source defaults

The protected location-management family uses `partyId` and, for existing sites,
`locationId` alongside the shared REST body/query fields. `createPartyLocation`,
`correctPartyLocation` and `setPartyLocationLifecycle` require `parties:write`;
`listPartyLocations` and `getPartyLocation` require `parties:read`. The existing use cases
recheck explicit location-management grants or eligible active partnership membership.
Party CRUD and manager-grant changes are excluded.

Creation preserves label, every role, management geography/position, scoped evidence and
optional `relocates: { id, revision }`. It is keyless: identical requests can create distinct
records. Its idempotent annotation is false, and clients must reconcile after an ambiguous
timeout before deciding whether to create again. Relocation atomically retires the old site
and creates its replacement; repeating the old revision conflicts without recovering the
successful result. See [command retries](command-retries.md).

Correction retains `expectedRevision` and `correction: "SAME_SITE"`. Omission preserves a
field; null clears geography, position or evidence, while label and roles reject null.
Lifecycle changes use the existing revision and `ACTIVE`/`RETIRED` values. All management
responses retain revision, inputRevision and evidence. Listing accepts `after` and `limit`
(default 25, 1–100), returns `{ items, next }`, and uses the returned `next` as `after`.
An exhausted page returns `next: null`; authority is checked again on every page.

`getSourceLocationDefault` requires `listing-sources:read`. `assignSourceLocationDefault`,
`changeSourceLocationDefaultApplicability` and `clearSourceLocationDefault` require
`listing-sources:write`. Arguments combine `listingSourceId` with the shared REST body.
The services require current source authority, exact source grants for eligible partnership
operators, and an operator-owned active location for assignment/applicability changes.
Business-association and advertised-inventory evidence retain separate scopes. Applicability
omission preserves that meaning; null clears it. Clear remains valid after retirement and
returns `assignment: null`, `eligibility: null` with the retained revision tombstone.

Revision-checked mutations advertise idempotent effects: a repeated committed revision
can conflict and does not replay the successful result. No mutation is automatically retried.
Management evidence and exact inputs remain protected and are never included in public
discovery contracts or operational logs. The [family acceptance tests](../src/aura-historia-mcp/tests/location_management.rs)
exercise production composition, current grants, paging, evidence and revision behavior
against isolated local PostgreSQL.

## Generic OAuth acceptance and stage smoke

Register one fixed confidential client per environment with an exact HTTPS callback and a
scope ceiling, then follow the [generic OAuth setup](oauth.md#fixed-confidential-client-setup).
The same MCP URL supports anonymous discovery and public calls before, during and after a
connection. Protected tools remain listed; their HTTP `401`/`403` challenges identify the
resource metadata and required scopes. To add scopes, start a new authorization with a fresh
state and S256 verifier, obtain human approval and exchange its single-use code. A client
must validate the exact callback and state, handle `access_denied` without exchanging a
code, and keep its secret and verifier out of browser URLs and logs.

The [local generic acceptance fixture](../src/aura-historia-api/tests/api_cases/mcp_oauth/mod.rs)
drives actual OAuth/REST HTTP routes, native MCP HTTP and the actual Lambda adapter against
isolated local PostgreSQL. It checks the disjoint manifests against `tools/list`, validates
published schemas and compares account schemas to the shared API contracts. Run it with Docker
and the repository's test infrastructure available:

```sh
cargo test --locked -p aura-historia-api --test api \
  mcp_oauth::should_verify_generic_oauth_and_real_hybrid_mcp -- --nocapture
```

That fixture constructs the advertised browser handoff and exercises backend approval with
test first-party identity. It does not render or test the webapp. Local fixtures, compatibility
tests and CDK synth establish local evidence only; stage/browser/released acceptance needs
its own executed evidence.

After the approved stage release and DNS/TLS/front-door checks in the
[hosting runbook](http-api-front-door.md#mcp-deployment-and-stage-verification), run the
optional read-only command from the matching release checkout (Python 3 on Linux/macOS):

```sh
python3 tools/mcp-smoke.py --release-commit <exact-deployed-40-character-sha> \
  --output /tmp/mcp-stage-evidence.json
```

This command contacts only the canonical stage MCP/issuer origins, follows no redirects,
performs at most 40 requests, caps each response at 2 MiB and stops within 90 seconds. It checks
both resource-metadata paths, AS discovery, supported current/legacy revisions, the actual
manifest union, a public operation, HTTP auth failures, Origin/CORS/routing behavior and
no-store responses. It prints no bodies or bearer values. The evidence records the deployed
commit supplied by the operator, runner commit, SDK/protocol versions, statuses and selected
public header values; challenge values and unexpected header values are omitted. The command
checks JSON-RPC framing, manifest policy, matching tool representations and their advertised
required members. The local Rust fixture validates selected complete outputs against the shared
DTO schemas. A failed or omitted check is not a pass.

For the delegated read check, complete generic consent for a dedicated stage user with
`users:read`, place the returned token in an owner-readable file outside the checkout,
and add `--token-file /path/to/stage-token`. The optional request reads the user's account
but does not record its result. Revoke the dedicated connection afterwards through Aura's
existing account token controls or `/v1/oauth/revoke`; do not include secrets in the evidence.
The command performs no registration, approval, exchange, mutation, revocation or deployment.
Browser consent/resource forwarding and discoverable first-party revocation controls remain
webapp-owned release checks. Live branded-client setup is optional.

## Bounds, metadata and runtime configuration

The default admission bounds are 256 KiB per request, 2 MiB per complete JSON
response and 64 concurrent requests per process. The complete response includes structured
content, duplicate JSON text for generic clients and protocol framing. A 32–64 KiB ordinary
response target is not a hard result limit, and client limits can differ. Library composition
may lower these bounds. Body admission counts streamed bytes regardless of
`Content-Length`; oversized input receives `413`. Output serialization stops
at the limit and returns an explicit failure. With a 2 MiB JSON body, escaping
that text inside the Lambda response envelope remains below the 6 MiB Lambda
synchronous response limit. The actual Lambda adapter is exercised with escaped
output in [`lambda.rs`](../src/aura-historia-mcp/tests/lambda.rs).

With the pinned SDK, the local public fixtures preserve these complete responses above 64 KiB:

| Local fixture | Complete JSON body | Lambda envelope |
| --- | ---: | ---: |
| Detail with 175 long image URLs | 201,666 bytes | 204,409 bytes |
| Unpaginated history with 128 events | 114,848 bytes | 135,921 bytes |

The request deadline is 25 seconds at most, including credential refresh/router composition on
Lambda, body admission and the authentication gate. Lambda caps this by its remaining invocation
time with one second reserved for response handling. Exhausted budgets receive HTTP
`408`; work gets a cancellation token and an absolute deadline through
`InvocationContext`. Native HTTP disconnects cancel active work; Lambda cannot
observe a gateway client disconnect, so its deadline bounds that work. Separate
legacy cancellation notifications do not locate work in another invocation.
Application wrappers must propagate the supplied deadline/token to external
work and must not detach unbounded tasks.

Requests get a new request ID (the invocation ID on Lambda) and a correlation ID.
An optional `X-Correlation-Id` is accepted only as a nonempty ASCII identifier
of at most 128 letters, digits, dashes, underscores or periods. These values
reach the shared `OperationContext` and successful responses carry both headers.
All HTTP responses and authentication challenges use `Cache-Control: no-store`.
MCP catalogue/discovery cache hints remain private with zero lifetime.

| Environment variable | Meaning |
| --- | --- |
| `OAUTH_ISSUER_URL` | Required exact Aura issuer: `https://api.aura-historia.com` or `https://api.stage.aura-historia.com`; selects the matching shared resource policy |
| `MCP_BIND_ADDR` | Native listener; defaults to `127.0.0.1:8080` |
| `MCP_ALLOWED_HOSTS` | Comma-separated exact allowed HTTP authorities; defaults to `localhost:8080,127.0.0.1:8080` |
| `MCP_ALLOWED_ORIGINS` | Comma-separated exact HTTP origins; unset rejects every present Origin |

Configure the public hostname explicitly for Lambda. Missing Origin is accepted;
present Origin must be a valid allowed scheme/host/port tuple. No wildcard or
opaque `null` origin is supported. Default ports compare by their effective
value, so `https://app.example` and `https://app.example:443` agree; another port
does not. Invalid environment configuration prevents startup.

The binary suppresses all SDK logging with `rmcp=off`: SDK trace events include
private protocol payloads. Library embedders must retain that directive or an
equivalent filter even when enabling debug/trace logs elsewhere. Never log
Authorization values, protocol bodies, private arguments/results or technical
error causes containing provider data.

Native startup uses the shared explicit PostgreSQL configuration; Lambda refreshes PostgreSQL
credentials before every invocation and caches service composition by credential version.
Token authentication still reads authoritative state on every request. Infrastructure injects
the same explicit `OAUTH_ISSUER_URL` into REST and MCP; native startup must supply it too.
Refresh failures reject the invocation instead of reusing stale credentials; in-flight routers
retain their handles across replacement. Listing startup additionally requires
`OPENSEARCH_ENDPOINT_URL`, and `OPENSEARCH_USERNAME`/`OPENSEARCH_PASSWORD` outside `STAGE=ephemeral`.
`VERTEX_AI_PROJECT_ID`/`VERTEX_AI_LOCATION` and the existing `PRODUCT_LISTING_SEARCH_*` cache/read
settings use the [shared runtime configuration](../src/aura-historia-runtime/src/product_listing_config.rs).
ADC construction remains lazy until an embedding is requested. Lambda prepares injected
`AURA_HISTORIA_GOOGLE_ADC_CREDENTIALS_JSON` privately before starting threads; native execution
uses the configured Google ADC environment. No unrelated billing, newsletter or identity-provider
configuration is needed. Infrastructure supplies the same listing-reader/provider settings as
REST while retaining only the MCP runtime's existing PostgreSQL secret-read IAM permission.
