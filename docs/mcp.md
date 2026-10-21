# MCP transport

The canonical MCP endpoint is `https://mcp.aura-historia.com/v1`. The
`aura-historia-mcp` crate serves the same `/v1` router natively and through
Lambda HTTP API v2. The [front-door contract](http-api-front-door.md#mcp-front-door)
owns hosting, certificates, Origin/CORS and stage verification. Native and Lambda entry
points share one registry and authenticate each request independently. Configure a generic
remote MCP client with this URL and Streamable HTTP transport. Public catalogue discovery and
public calls require no account; protected operations use opaque Aura access tokens.

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
[public](../src/aura-historia-mcp/public-operations.json) and
[authenticated](../src/aura-historia-mcp/authenticated-operations.json) manifests together record
the exact allowlist, family mappings, access policy, annotations and exclusions. `tools/list`
publishes each callable operation's generated `inputSchema` and `outputSchema`. A closure
captures narrow service handles and calls the shared
inbound application use case directly. Use shared `aura-historia-contracts`
DTOs and constructors; transport wrappers may assemble path/query/body inputs
but must not duplicate business fields or business behavior.

Input schemas use Schemars deserialization direction; output schemas use
serialization direction. Inputs must be objects, including tagged unions whose
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

## Public catalogue

The public catalogue currently maps service calls to `Anonymous`, including calls with valid
Aura credentials; delegated personalization is activated separately by #2247. These operations
require no scopes and advertise read-only, idempotent, nondestructive behavior against Aura's
catalogue. These hints describe behavior; services remain the authorization authority. Private reads, writes and administrator operations
have no registration in this catalogue. Newsletter opt-in/confirmation, health/readiness,
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
Anonymous results omit `userState`; public source/location/auction output excludes management,
contact, grant and provider configuration. Unsigned amounts and opaque integer sort values remain
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
Local tests exercise both discovery and initialize lifecycles, all twelve tools, complete search
continuation, long image URLs, 128-event history and explicit oversize failures. These tests are
local verification; they do not establish deployment, DNS, WAF behavior or a client-specific bound.

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
