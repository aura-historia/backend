# MCP transport

The canonical MCP endpoint is `https://mcp.aura-historia.com/v1`. The
`aura-historia-mcp` crate serves the same `/v1` router natively and through
Lambda HTTP API v2. The [front-door contract](http-api-front-door.md#mcp-front-door)
owns hosting,
certificates, Origin/CORS and stage verification. Its production registry is empty;
public business-tool releases supply their selected operation families.

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
four mutation/retry/open-world annotations. Release composition assembles the
allowlist. A closure captures narrow service handles and calls the shared
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

`AdmissionGate` runs before SDK operation dispatch. It receives the bounded
body's method and selected registered operation only after supplied routing
headers agree. The future authentication gate must validate supplied tokens
through application services on every request and preserve the exact
`DelegatedUser` capabilities. Services remain the authorization authority.
The foundation gate accepts anonymous requests, rejects supplied credentials,
and rejects protected calls with HTTP `401`. A gate can return actual HTTP
`401`/`403` and `WWW-Authenticate` before tool-result wrapping. It must not
infer argument authorization from unvalidated `Mcp-Param-*` headers.

Family adapters classify expected input, missing-resource, conflict, forbidden
and temporary failures into distinct static tool-error codes. Invalid arguments
are protocol errors; unexpected failures are safe `-32603` errors. Technical
causes remain attached to internal error types and never appear in client
messages. No mutation is automatically retried: a timeout or oversized response
may follow a completed write, so callers must follow the operation's existing
idempotency and recovery contract.

## Public listing family

The module-local [listing registrations](../src/aura-historia-mcp/src/product_listings/mod.rs)
provide these public read operations for release composition; the production registry remains
empty until the public release selects its families.

| Tool / source operation ID | Arguments | Result |
| --- | --- | --- |
| `simpleSearchProductListings` | Complete existing filters plus `sort`, `order`, `size` and `searchAfter` | Existing cursored summary response, including totals when supplied by the service |
| `getProductListing` | `productListingId`, language and currency | Existing personalized detail envelope; anonymous results omit `userState` |
| `getProductListingByTitleSlug` | `productListingTitleSlugId`, language and currency | The same detail envelope |
| `getSimilarProductListings` | `productListingId`, language and currency | `{ "status": "EMBEDDING_PENDING" }` or `{ "status": "READY", "items": [...] }` |

Search preserves geography, shipping, all identifier inclusion/exclusion sets, price and date
ranges, availability, orderability and unspecified-availability predicates. Presentation defaults,
size clamping and FX-bound continuation follow the shared contract. `searchAfter` is the existing
cursor object supplied directly as JSON; REST alone encodes that object into its query string.
Unsigned amounts, revisions and opaque integer sort values remain exact JSON integers.

The similarity envelope represents REST's bodyless pending HTTP outcome. Pending means the
embedding is unavailable and callers may retry the same operation later; `READY` with an empty
array is a completed result. The service owns the fixed neighbor limit, so no `size` argument
is offered. All four tools use native inbound use cases and the same shared DTOs and pure
mappings as REST, preserving localization/fallback, source-local identifiers, outbound URLs,
shipping disclosure, content policy and withdrawn-listing handling. Listing input schemas and
output schemas derive from those shared types, including explicit custom-codec annotations.

## Public listing history

The listing-history family registers `getProductListingHistory` as a public, read-only,
idempotent operation with no required scopes. Its arguments are `productListingId` and
optional `currency`; omission or null leaves source prices unchanged. It calls the existing
complete-history use case with the request's anonymous context. Release composition selects
this module alongside the other public families.

REST keeps its full JSON array. MCP uses the same shared history entries inside
`{ "result": [...] }`, including discovery and change details, event IDs, timestamps, source
amounts and optional historical display prices. Events retain their original order, including
distinct events with matching timestamps. Each priced event retains its own historical FX
snapshot and provenance; missing historical snapshots or quotes omit the display projection
without substituting a later or current rate. Existing empty histories succeed; unknown
listings return `NOT_FOUND`.

History has no paging arguments, item limit or continuation. The complete result must fit the
transport's byte bound; an oversized result fails explicitly and returns no partial history.

## Public source, location and overview family

The `public_discovery` family registers four anonymous, read-only, idempotent operations.
Release composition selects the production allowlist.

| Stable operation ID | Arguments and result |
| --- | --- |
| `searchPublicListingSources` | `query`, `size` (default 21, 1–50) and opaque `searchAfter`; returns the existing public source collection. Absent/blank text browses; insufficient text returns the service's empty result. Continuations are bound to the canonical query and page size. |
| `getPublicListingSourceBySlug` | `listingSourceSlugId`; returns the public source card, including operator name and optional URL/image. |
| `discoverPartyLocations` | Complete `geography` (`countries`, `subdivisions`, numeric `origin`, `radius`, `mode`, `distanceOrder`), `size` (default 21, 1–100) and `searchAfter` (`locationId`, optional `distanceKm`); retains IDs, basis, geographic context, point, precision and distance. |
| `getPublicOverview` | Empty object; returns `asOf` and every stored public catalogue aggregate. An uncalculated overview is temporarily unavailable. |

The family reuses shared contracts and pure converters with REST and calls the existing inbound
services directly. Source cursors retain their 4 KiB encoded/3 KiB decoded bounds for JSON tool
arguments. REST alone handles raw URL-query bytes. Location geography and cursor validation remain
with the existing constructors and service; no address/profile origin is inferred. Public results
exclude management evidence, grants, contacts and provider configuration.

Source search and slug detail share the runtime's non-queuing read budget. Capacity exhaustion,
source timeout, invocation cancellation/deadline and temporary service failure produce retryable
`TEMPORARILY_UNAVAILABLE` tool errors; active work is dropped and permits released. Invalid semantic
inputs/cursors receive `-32602`, missing sources receive `NOT_FOUND`, and invalid stored results
receive a safe `-32603`. The outer HTTP deadline keeps its documented `408` admission behavior.


## Anonymous auction family

The module-local [auction registrations](../src/aura-historia-mcp/src/auctions/mod.rs)
provide `listAuctions`, `getAuction` and `getAuctionCatalogue` for release composition.
They call the existing inbound auction and catalogue use cases with `Anonymous`.
The [shared auction contracts](../src/aura-historia-contracts/src/auctions/mod.rs)
own both REST wire DTOs and generated MCP schemas, including the schedule used by
administrator auction output. Production admission and registration belong to the
public release composition.

`listAuctions` retains source, format, reported status, schedule range, sort/order and
pagination inputs. A scheduled sort requires `timeRole`; a time range requires both
bounds. Directory cursors bind the complete filter and sort scope, including nullable
schedule instants. `getAuction` retains source metadata, schedule, reported status/counts,
visible count and catalogue/outbound links without administrator keys or audit state.
`getAuctionCatalogue` combines `auctionId` with the existing language, currency,
`pageSize` and `searchAfter` inputs and reuses listing localization, image redaction,
shipping presentation and user-state omission.

For directory and catalogue continuation, serialize the returned `searchAfter` object
as the next input's JSON string, as on REST. Catalogue cursors retain auction identity,
catalogue position and listing identity; the use case retains its current/sale-observation
FX selection. Malformed or wrong-scope cursors are invalid arguments, missing auctions
are `NOT_FOUND`, and pricing or storage outages are explicit temporary failures.
The [contract tests](../src/aura-historia-contracts/tests/auctions.rs) and
[operation tests](../src/aura-historia-mcp/src/auctions/tests.rs) cover these mappings.

## Bounds, metadata and runtime configuration

The default admission bounds are 256 KiB per request, 2 MiB per complete JSON
response and 64 concurrent requests per process. Library composition may lower
these bounds. Body admission counts streamed bytes regardless of
`Content-Length`; oversized input receives `413`. Output serialization stops
at the limit and returns an explicit failure. With a 2 MiB JSON body, escaping
that text inside the Lambda response envelope remains below the 6 MiB Lambda
synchronous response limit. The actual Lambda adapter is exercised with escaped
output in [`lambda.rs`](../src/aura-historia-mcp/tests/lambda.rs).

The request deadline is 25 seconds at most, including body admission and the
authentication gate. Lambda caps this by its remaining invocation time with
one second reserved for response handling. Exhausted budgets receive HTTP
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
error causes containing provider data. The empty production catalogue needs no
business-service credentials; families inject their own runtime handles.
