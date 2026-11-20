# HTTP API front door

This document owns edge routing, authentication, forwarding and cache/security rules.
[OpenAPI](swagger.yaml) owns REST request/response contracts; [MCP](mcp.md) owns
protocol compatibility and tool admission. [ProductListing](product-listing.md),
[geography](geography.md) and [marketing consent](marketing-consent.md) own use-case semantics.
Source/synth do not verify live DNS, certificates, WAF, the host proxy or forwarding;
operators must inspect them.

## Route and authentication boundary

CloudFront forwards every path to the host; Axum owns the route boundary and answers
unknown paths itself. [`api-route-parity.test.ts`](../infra/test/api-route-parity.test.ts)
keeps the Axum routes identical to the OpenAPI operations: canonical `/v1` paths and exact
well-known documents, with no aliases or catch-all routes.

The edge deliberately applies **no Cognito JWT authorizer**. Axum validates Cognito
access JWTs and Aura opaque access tokens, optional bearers, delegated scopes and
required user/admin/partner authorization. Anonymous probes remain anonymous;
optional-auth routes reject invalid supplied credentials. OAuth and signed-provider
handlers retain their dedicated credential/proof checks. A Cognito-only edge policy
would reject valid application credentials before those handlers can evaluate them.

Cognito JWT authentication validates signatures, issuer, client, token use and expiry
locally, then checks the session cutoff returned by the existing identity lookup.
Signed integer `auth_time` and `iat` claims are required. `auth_time` must be nonnegative
and no later than `iat`; `iat` cannot lead the API clock by more than 60 seconds.
A session authenticated at or before its stored cutoff is rejected. Token refresh
preserves `auth_time`, so it cannot restore a revoked session. Cutoffs are never cached.
Ordinary requests make no live Cognito status call. No additional Cognito scope is required.

Each API instance caches Cognito signing keys for five minutes. When a JWKS fetch
completes, fails or is abandoned by a cancelled request, a ten-second cooldown starts in
which unknown key IDs are rejected and expired keys are served without fetching, so
neither forged key IDs nor provider failures cause more than one fetch per cooldown;
concurrent misses share one fetch. A key Cognito publishes is therefore accepted within
about fifteen seconds: the cooldown plus the five-second fetch timeout of a fetch that
started just before publication. While refreshes fail, previously fetched keys stay
usable; a successful refresh that omits a key stops accepting it. Bearers whose key is not
cached fail closed with `503` until the cooldown after a failed fetch ends, including on a
newly started instance. Malformed key material counts as a failed fetch.

Administrator session revocation commits a local cutoff before Cognito global sign-out,
then advances it again after the provider call returns, including when the provider
returns an error. Both writes use the database clock plus a 60-second skew allowance
and never move backward. Local revocation can therefore take effect even when the
endpoint returns an error. Fresh logins may be rejected for roughly a minute after
the last completion/retry; issuer, API and database clocks must remain synchronized
within that allowance. No database transaction spans provider I/O.

This enforces revocations requested through this API. Changes made only in Cognito
(including provider-only global sign-out or refresh-token revocation) are not observed
immediately by this API; already-issued JWTs can remain accepted until expiry. Use the
administrator endpoint when immediate local session revocation is required. Database
failures fail closed; opaque Aura tokens keep their existing persisted checks.

Preserve request headers, cookies, query strings and exact signed bodies through the
edge and the host proxy, including JSON bodies on DELETE. Do not rewrite webhook
payloads or move one-time confirmation into GET/redirect processing. Axum answers
preflight and sets response CORS; it must retain required authorization/provider headers,
`Idempotency-Key` admission/exposure and `WWW-Authenticate` and `Retry-After` exposure. REST CORS allows any Origin (`*`),
including preview hosts, for browser requests that do not use
`credentials: "include"`. Clients supply bearer tokens explicitly in the
`Authorization` header. CORS does not grant application authority; all existing
authentication, provider proof, authorization, raw-body limits and application checks
remain in force. Header forwarding does not opt an operation into replay; preserve
the [keyless creation exceptions](command-retries.md). MCP uses a separate,
explicit origin allowlist.

## Front-door controls

- The public hostname is exactly `api.aura-historia.com`. The canonical REST prefix is `/v1`,
  without an origin-path rewrite.
- CloudFront redirects viewers to HTTPS and uses the HTTPS-only host origin, **not the
  public CloudFront-facing DNS record**. AllViewer forwards `Host`, so the host proxy serves
  the public hostname's certificate and routes by it. Every origin request carries the
  secret `X-Origin-Verify` header, and the host accepts only CloudFront's origin-facing
  ranges; see the [origin lock](infra.md#cloudfront-origin-lock).
- Retain CloudFront-scoped AWS IP reputation, common-rule-set and known-bad-input
  rules; `NoUserAgent_HEADER` and `SizeRestrictions_BODY` are count-only. There is no
  WAF rate rule. The application owns request-rate limits, which shape requests, not
  database capacity. Routes that make a paid provider call per request, such as
  [address suggestions](geography.md#address-suggestions), carry their own lower limit
  as a ceiling across all callers; per-user quotas stay in the application. Each API
  process enforces a token bucket for every request (`AURA_HISTORIA_API_RATE_LIMIT_PER_SECOND`,
  `AURA_HISTORIA_API_RATE_LIMIT_BURST`; defaults 1,000 per second, burst 2,500) and a lower one
  for `POST /v1/address-suggestions` (`AURA_HISTORIA_API_ADDRESS_SUGGESTIONS_RATE_LIMIT_PER_SECOND`,
  `..._BURST`; defaults 12.5 per second, burst 25). The limits are per process, so Caddy's
  two API replicas together admit twice the defaults; change them together with the replica
  count. An exhausted limit returns `429 REQUEST_RATE_LIMITED` with `Retry-After: 1` before
  authentication, with the usual CORS and request headers. Only the liveness probe
  `/v1/health` and the signed Cognito trigger routes under `/internal/cognito/` are exempt:
  Cognito cannot retry a trigger, so a flood must not fail sign-ups, and each trigger request
  is authenticated by its signature before it reaches the database. `/v1/ready` queries
  PostgreSQL and OpenSearch, so it stays limited.
- Contract-valid loopback redirect URIs can match the common rule set's EC2 metadata
  SSRF body/query rules (`localhost`, such as the Shopify CLI `--use-localhost` callback
  `https://localhost:3458/oauth/callback`, or a native agent's `http://localhost:…/callback`)
  and, as IPv4 literals, its RFI body/query rules; so can URL-valued OAuth `client_id`s.
  Those four rules are counted in the group; a label rule after it blocks their matches
  everywhere except the exact OAuth client registration/update, authorize, token and
  metadata-document consent (`GET /v1/oauth/metadata-document-clients`) paths and the exact
  content-notice submission path (`POST /v1/content-notices`), whose notices may cite reported
  content at raw IP addresses. The API never fetches a redirect URI or a cited URL submitted
  there. It fetches only a URL-valued `client_id`'s metadata document, through the dedicated
  outbound policy in the [OAuth contract](oauth.md#metadata-document-clients):
  HTTPS on port 443 to hosts resolving exclusively to public addresses, without redirects.
  That application control replaces the WAF URL-target rules on these paths. Their
  cookie/URI-path variants and every other managed rule still apply. Never widen this scope
  to another route that fetches a user-supplied URL; every outbound fetcher enforces the
  shared public-network policy. CloudWatch label metrics and sampled requests show which
  counted rule matched. The Web ACL has five rules, the Free plan quota; consolidate before
  adding one.
- Edge and application body limits must agree. The managed 8 KiB body-size rule is
  below contract-valid JSON bodies, so a custom WAF rule instead blocks a declared
  `Content-Length` above the Axum whole-request limit with the API's JSON `413`
  `BAD_BODY_VALUE`; [`api-web-acl.test.ts`](../infra/test/api-web-acl.test.ts) pins
  the two limits together. WAF cannot inspect beyond its body-inspection window or
  bound undeclared lengths, so Axum remains the authoritative limit, and managed body
  rules evaluate only the inspected prefix. Typed request validation remains the
  control for the rest of the body. The rule covers every path, provider webhooks
  included, so no route may admit a larger body than the edge. Stay within the
  CloudFront plan's WAF rule quota.

## Selective anonymous discovery caching

Only explicitly approved anonymous `200` discovery representations may be shared.
Any `Authorization` header disables shared caching, including malformed credentials;
discovery responses use `private, no-store`.
Unmarked responses and errors default to no-store; pending results, redirects and
non-`200` responses are not cacheable. Consent, OAuth, protected and write routes
remain uncached. Application cache approval and edge policy must agree.

| Approved GET representation | Shared TTL |
| --- | ---: |
| Listing-source collection and by-slug detail | 300 s |
| Auction collection, detail and auction product-listings | 60 s |
| Product-listing collection | 60 s |
| Product-listing ID/by-slug detail | 120 s |
| Product-listing history and ready similar results | 300 s |
| Public catalogue overview | 900 s |
| [API catalog](agent-discovery.md#api-catalog), including `HEAD` | 300 s |
| [Website discovery documents](agent-discovery.md#website-documents), including `HEAD` | 300 s |

Approved responses use `public, max-age=0, s-maxage=<TTL>, stale-if-error=0`. Listing
responses containing advertised shipping cap this TTL at referenced geographic provider
evidence retention deadlines; shipping observation age does not expire terms. Destination
personalization remains private and uncached.
The custom policy has zero minimum/default TTL, keys on `Authorization`, `Origin`,
`Host` and **all query strings**, and enables gzip/Brotli variation. Cookies are
forwarded but not keyed: never add cookie-dependent data to a shared representation.
OPTIONS is not cached; no stale-while-revalidate or positive stale-on-error window exists.

Only reviewed discovery path behaviors, including the exact `/v1/overview` and
`/.well-known/api-catalog` paths and `/v1/site-discovery/*`, whose routes are all exact anonymous
documents, use this policy; generic `/v1/*` and default behaviors use `CachingDisabled`. Wildcard
expansion requires route/cache review and tests. Do not add token-derived keys, cookie
partitions or an edge auth function.
Cache-enabled behaviors strip `X-Request-Id` and `X-Correlation-Id` on both hits and
misses so one viewer cannot receive another's origin IDs. Uncached behaviors retain
them. Configured CloudFront error TTLs are zero; application errors are also no-store.

### OAuth authorization-server metadata

The exact public `GET /.well-known/oauth-authorization-server` route is an OpenAPI operation
and requires neither bearer nor client credentials. It uses the
CloudFront default cache-disabled behavior and origin `no-store` headers. OAuth credential
responses and protocol errors use `no-store` and `Pragma: no-cache`. The exact public
`GET /.well-known/oauth-protected-resource` route is handled the same way; the
[agent discovery contract](agent-discovery.md#rest-protected-resource-metadata) owns it and
the Bearer challenges that point to it. The shared deployment `OAUTH_ISSUER_URL` configures both API and MCP
and selects the same closed resource policy. Discovery, protocol errors and fixed-client
setup are owned by the [OAuth contract](oauth.md).

Metadata requires no URI-bearing request input and receives no WAF exemption. OAuth
`resource`, callback URIs and URL-valued client IDs continue through the exact
authorization, token and metadata-document consent path exceptions; unrelated paths retain
the counted URL-target blocks. Revocation and the account disconnect
(`DELETE /v1/me/access-tokens?clientId=…`) carry only a client ID, which names a domain and
never an IP literal or callback, so they need no exception and keep those blocks.

## MCP front door

MCP uses a separate server process, CloudFront distribution and Web ACL on the same locked
host origin. Its canonical endpoint is `https://mcp.aura-historia.com/v1`. The REST host and
routes are independent. [`mcp.ts`](../infra/src/constructs/mcp.ts) declares the distribution;
the MCP server answers exact paths only, and `404` for any other:
`/v1` forwards methods to the native transport, including `GET`/`HEAD`/`DELETE`
rejections (`405`, `Allow: POST`); `/health` and `/ready` admit `GET`/`HEAD` (`204`).
Four further exact `GET`/`HEAD` paths serve discovery documents:
`/.well-known/oauth-protected-resource` and `/.well-known/oauth-protected-resource/v1` the
[protected-resource metadata](mcp.md#operation-families-and-admission),
`/.well-known/openai-apps-challenge` the
[OpenAI domain verification token](mcp.md#directory-metadata), and
`/.well-known/mcp/server-card.json` the [server card](agent-discovery.md#mcp-server-card).
While the deployment configures a [Muse token](mcp.md#directory-metadata), the exact path
`/.well-known/muse-challenge/<token>` is a fifth. No proxy,
REST handlers, sessions or SSE service are exposed on this host.

The origin has no path rewrite. CloudFront
forwards all viewer headers, query strings and cookies using AllViewer, including
`Host`, `Authorization`, `Origin`, `Accept`, `Content-Type`, `MCP-Protocol-Version`,
`Mcp-Method`, `Mcp-Name`, `Mcp-Session-Id` and `X-Correlation-Id`. Preserve response
status, `WWW-Authenticate`, `Allow`, content type and request/correlation IDs.
Every other MCP behavior uses CachingDisabled, zero error TTL and a `Cache-Control:
no-store` response policy, including proxy-generated errors and preflight. No
headers or status codes are rewritten to hide authentication/protocol errors. The
server card's exact-path behavior is the only cached one: it honors the card's
five-minute origin TTL, keys on the path alone and neither keys nor forwards credentials,
so discovery traffic stays off the MCP server. Its response policy adds `no-store`
only where the proxy generated the response without one.

The MCP server owns browser preflight and validates Origin against its exact list. The public
server card is the exception: its behavior's CloudFront CORS policy replaces the server's CORS
headers with `Access-Control-Allow-Origin: *`, and the native route skips the Origin check.
Prod permits `https://aura-historia.com`. There are no wildcard
or opaque `null` origins, cookies/credential-mode CORS is not enabled, and normal
server clients may omit Origin. A browser uses bearer Authorization explicitly;
CORS grants no application authority. Requests may use the forwarding headers above;
responses expose `WWW-Authenticate`, `X-Request-Id`, `X-Correlation-Id` and
`Content-Type`. Preflight has zero max-age. An unapproved Origin on an actual
protocol request is rejected by the transport; a denied preflight must not grant
an allow-origin response.

The dedicated Web ACL keeps IP reputation, common and known-bad-input protections.
`NoUserAgent_HEADER` is count-only for server clients. The managed 8 KiB body-size
rule is counted and re-blocked outside **exact `POST /v1`**; no URL-target or other
managed-body rule is relaxed for MCP. A declared body over 256 KiB receives JSON
`413` with no-store before managed inspection. WAF inspects only its configured
prefix and cannot bound undeclared/chunked lengths; the transport's streamed-byte
limit is authoritative. This exception admits JSON-RPC above the managed threshold,
without promising that arbitrary payload content bypasses managed rules. WAF request
sampling and payload logging are disabled for MCP. Review metrics without recording bearer
headers or arguments/results. The [hosting tests](../infra/test/mcp-hosting.test.ts)
pin these boundaries to the transport budgets.

### MCP deployment and stage verification

Deploy CDK changes as described in [deploying the reduced app](infra.md#deploying-the-reduced-app).
Before first traffic, verify domain ownership, then provision an issued
ACM certificate covering the exact host in `us-east-1` (CloudFront viewer). Supply its ARN
through the external SSM String parameter `/certificates/prod/mcp-cloudfront-certificate-arn`
in the workload region. The host proxy holds its own certificate for the same hostname.
DNS remains operator-owned. Point the exact public hostname's CNAME (or provider alias) to
the API stack's `McpCloudFrontDistributionDomainName` output, never to the public hostname
itself or the origin name. Keep the previous DNS target/TTL for recovery. Inspect origin TLS
with the forwarded public Host and the secret header before DNS cutover.

After a deploy that changes the MCP server or its front door, exercise the **public
CloudFront hostname** with a generic Streamable HTTP client. For a manual legacy protocol probe:

```sh
mcp_origin=https://mcp.aura-historia.com
curl -i "$mcp_origin/ready"
curl -i "$mcp_origin/v1" \
  -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' \
  --data '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"smoke","version":"1"}}}'
curl -i "$mcp_origin/v1" \
  -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' \
  -H 'MCP-Protocol-Version: 2025-11-25' \
  --data '{"jsonrpc":"2.0","method":"notifications/initialized"}'
curl -i "$mcp_origin/v1" \
  -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' \
  -H 'MCP-Protocol-Version: 2025-11-25' \
  --data '{"jsonrpc":"2.0","id":2,"method":"tools/list"}'
```

Require readiness `204`, a supported initialization result and successful discovery.
Compare `tools/list` with the release's disjoint public/authenticated manifests and
call one actual listed public tool with bounded inputs and verify its complete
result. The [generic OAuth/MCP runbook](mcp.md#generic-oauth-acceptance-and-stage-smoke)
provides a bounded optional command and a separate delegated-read check. Complete
these checks before approving traffic:

| Probe | Required result |
| --- | --- |
| `GET`, `HEAD`, `DELETE /v1` | `405` and `Allow: POST`, no SSE or session establishment |
| `GET`/`HEAD /health` and `/ready` | `204`; readiness confirms the shell, not future tool dependencies |
| Both `/.well-known/oauth-protected-resource` paths (root and `/v1` suffix) | Public `200` metadata identifies the exact MCP resource and Aura issuer; no-store |
| `GET`/`HEAD /.well-known/openai-apps-challenge` | `200` `text/plain` with exactly the configured token, or `404` while none is configured; no-store |
| `GET`/`HEAD /.well-known/muse-challenge/<token>` | With a configured token: `200` `text/plain` with exactly `muse-challenge=<token>`, no redirect, no-store; any other token is `404` |
| `GET`/`HEAD /.well-known/mcp/server-card.json` with a foreign Origin | `200` `application/mcp-server-card+json`, `Access-Control-Allow-Origin: *`, `Cache-Control: public, max-age=300`; the production `/v1` remote, the `serverInfo` title and release version, and the supported protocol versions |
| API `GET`/`HEAD /.well-known/api-catalog` | `200` RFC 9727 Linkset with production URLs only and `Link: <…/.well-known/api-catalog>; rel="api-catalog"`; a repeat is an edge hit; with `Authorization`, `private, no-store` |
| API `GET`/`HEAD` on each [website document](agent-discovery.md#website-documents) below `/v1/site-discovery/`, with a foreign Origin | `200` with the document's media type, `Access-Control-Allow-Origin: *` and production URLs only; a repeat is an edge hit; the index digests match the served skills; an unlisted skill name is `404` no-store |
| Once the webapp publishes it, the [WebMCP client](oauth.md#the-websites-webmcp-client) `https://aura-historia.com/oauth/webmcp-client.json` as `client_id` | The website answers `200` `application/json` without a redirect; consent metadata loads, so Aura's own fetch passed the outbound policy and Cloudflare bot protection; approval returns a code to the website's callback; the code exchanges for an access and a refresh token; the access token calls a protected MCP tool |
| API `GET /.well-known/oauth-protected-resource`, then `GET /v1/me/account` without a bearer | `200` metadata for `https://api.aura-historia.com`, no-store; the `401` carries `WWW-Authenticate: Bearer resource_metadata="…"` naming that metadata |
| An unrelated `/v1/me` path | `404`, no REST response |
| Allowed-origin POST and OPTIONS preflight requesting Authorization and all MCP headers | Exact allow-origin and required header admission/exposure; no wildcard |
| Unapproved or `null` Origin, unsupported version, or mismatched `Mcp-Method`/body | Rejected before dispatch; denied preflight grants no allow-origin |
| Wrong Content-Type, unacceptable Accept, duplicate routing/credential headers | `415`, `406`, or `400`; no dispatch |
| Valid JSON-RPC body above 16 KiB, below 256 KiB | Reaches protocol handling; not the managed 8 KiB size rejection |
| Body above 256 KiB with declared and streamed/undeclared lengths | `413`; no truncation or successful dispatch |
| Repeated success, `404`, `405`, protocol failure, supplied invalid bearer, and preflight | `Cache-Control: no-store`, no shared cache hit; status and challenge/trace headers survive |

Also run the [current revision flow and compatibility matrix](mcp.md#protocol-compatibility)
through a generic client. Record safe status/timing/header evidence without tokens or
bodies. Local synth/unit tests do not perform these live checks. If certificate,
Host/TLS, WAF, CORS or forwarding checks fail, hold traffic. Restore the recorded DNS
target when necessary; code rollback is a [deploy of an earlier commit](../host/prod/README.md#deploys). DNS is never
rolled back automatically, and rollback does not undo completed tool effects.

## REST client and provider configuration

| Environment | REST base URL |
| --- | --- |
| Production | `https://api.aura-historia.com/v1` |

Append operation suffixes exactly once, for example `/health` to the REST base URL.
Clients configured with an origin alone must include `/v1` in their request paths.
The removed `/api/v1` routes have no compatibility aliases, redirects or deprecation
period. Historical changelog entries retain their original paths.

## Front-door changes and rollback

Follow [deploying the reduced app](infra.md#deploying-the-reduced-app), then:

1. Inventory domain claims, externally owned DNS, current consumers and the previous
   endpoint/rollback target. Resolve conflicts with their owners.
2. Verify issued, valid certificates covering the **exact public hostname**: the
   CloudFront viewer certificate in ACM `us-east-1` and the host proxy's own certificate
   ([Caddy](../host/prod/README.md#caddy)). A wildcard covers one label only. Do not infer
   validity from synth.
3. Review the actual diff and the deployed commit, distribution, origin Host/TLS, origin
   lock, WAF and forwarding. Probe the approved full chain:
   health/readiness, anonymous and invalid optional bearers, allowed/denied protected
   Cognito/Aura calls, OAuth, CORS, signed bodies, a valid JSON body above 16 KiB
   reaching validation, an oversized `Content-Length` returning JSON `413`, and
   cache/header isolation. `/v1/ready` also proves that the API reaches PostgreSQL and
   [OpenSearch](opensearch-stage.md#production-host-setup).
   Keep credentials and signed payloads out of logs; omitted checks are not passes.
4. With approval and successful probes, update affected frontend/provider registrations
   and change external DNS only if its target changes. Repeat probes on the public hostname,
   including canonical `/v1` reads, representative writes and signed webhooks.

If DNS/certificate/Host/TLS/forwarding checks fail, hold the change. To roll back,
restore the recorded DNS target and affected client/provider URLs, accounting for TTL.
[Code/stack rollback](infra.md#rollback-and-stack-recovery) does not reverse DNS or
provider effects. Preserve WAF protections and retained state. A client, edge or proxy
timeout is not proof that side effects were canceled; reconcile before retrying writes.
