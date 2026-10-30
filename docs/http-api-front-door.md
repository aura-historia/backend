# HTTP API front door

This document owns edge routing, authentication, forwarding and cache/security rules.
[OpenAPI](swagger.yaml) owns REST request/response contracts; [MCP](mcp.md) owns
protocol compatibility and tool admission. [ProductListing](product-listing.md),
[geography](geography.md) and [marketing consent](marketing-consent.md) own use-case semantics.
Source/synth do not verify live DNS, certificates, aliases, WAF or forwarding; operators
must inspect them.

## Route and authentication boundary

[`api.ts`](../infra/src/constructs/api.ts) owns a closed Gateway route catalog, checked
against OpenAPI and Axum by [`api-route-matrix.test.ts`](../infra/test/api-route-matrix.test.ts).
There is no proxy or `$default` route. Ordinary REST methods target the `live` alias
of the single stage API Lambda; deploy its version/alias before the API stack uses it.
Preserve deployed route logical IDs when changing path-parameter spelling so
CloudFormation updates the existing routes rather than creating overlapping patterns.

Gateway deliberately applies **no Cognito JWT authorizer**. Axum validates Cognito
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
Lambda proxy integration, including JSON bodies on DELETE. Do not rewrite webhook
payloads or move one-time confirmation into GET/redirect processing. Gateway preflight
and Axum response CORS must retain required authorization/provider headers and
`Idempotency-Key` admission/exposure. REST Gateway CORS allows any Origin (`*`)
in both stages, including preview hosts, for browser requests that do not use
`credentials: "include"`. Clients supply bearer tokens explicitly in the
`Authorization` header. CORS does not grant application authority; all existing
authentication, provider proof, authorization, raw-body limits and application checks
remain in force. Header forwarding does not opt an operation into replay; preserve
the [keyless creation exceptions](command-retries.md). MCP uses a separate,
explicit origin allowlist.

## Front-door controls

- Public aliases are exactly `api.stage.aura-historia.com` for AWS **`dev`** and
  `api.aura-historia.com` for prod, never `*.stage.aura-historia.com`.
- Regional API Gateway uses TLS 1.2 and a default custom-domain mapping. The canonical
  REST prefix is `/v1`, without a mapping key or origin-path rewrite. The Lambda adapter
  normalizes a matching Gateway stage prefix; it must not strip arbitrary path segments.
  Keep `AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH=true`.
- CloudFront redirects viewers to HTTPS and uses an HTTPS-only regional Gateway
  origin (`attrRegionalDomainName`), **not the public CloudFront-facing DNS record**.
  AllViewer forwards `Host`; origin TLS and custom-domain mapping must accept it.
  The default execute-api endpoint is disabled when the custom domain is configured.
- Retain CloudFront-scoped AWS IP reputation, common-rule-set and known-bad-input
  rules; `NoUserAgent_HEADER` and `SizeRestrictions_BODY` are count-only. There is no
  additional WAF rate rule. Gateway stage throttles shape requests, not database or
  Lambda concurrency/capacity.
- Contract-valid loopback HTTPS redirect URIs can match the common rule set's EC2 metadata
  SSRF body/query rules (`localhost`, such as the Shopify CLI `--use-localhost` callback
  `https://localhost:3458/oauth/callback`) and, as IPv4 literals, its RFI body/query rules.
  Those four rules are counted in the group; a label rule after it blocks their matches
  everywhere except the exact OAuth client registration/update, authorize and token paths.
  The API never fetches URIs submitted there. Their cookie/URI-path variants and every
  other managed rule still apply on those paths. Never widen this scope to a route that
  fetches a user-supplied URL; outbound fetchers additionally enforce the shared
  public-network policy. CloudWatch label metrics and sampled requests show which counted
  rule matched. The Web ACL has five rules, the Free plan quota; consolidate before adding one.
- Edge and application body limits must agree. The managed 8 KiB body-size rule is
  below contract-valid JSON bodies, so a custom WAF rule instead blocks a declared
  `Content-Length` above the Axum whole-request limit with the API's JSON `413`
  `BAD_BODY_VALUE`; [`api-web-acl.test.ts`](../infra/test/api-web-acl.test.ts) pins
  the two limits together. WAF cannot inspect beyond its body-inspection window or
  bound undeclared lengths, so Axum remains the authoritative limit, and managed body
  rules evaluate only the inspected prefix. Typed request validation remains the
  control for the rest of the body. Stay within the CloudFront plan's WAF rule quota.

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

Approved responses use `public, max-age=0, s-maxage=<TTL>, stale-if-error=0`. Listing
responses containing advertised shipping cap this TTL at referenced geographic provider
evidence retention deadlines; shipping observation age does not expire terms. Destination
personalization remains private and uncached.
The custom policy has zero minimum/default TTL, keys on `Authorization`, `Origin`,
`Host` and **all query strings**, and enables gzip/Brotli variation. Cookies are
forwarded but not keyed: never add cookie-dependent data to a shared representation.
OPTIONS is not cached; no stale-while-revalidate or positive stale-on-error window exists.

Only reviewed discovery path behaviors, including the exact `/v1/overview` path, use
this policy; generic `/v1/*` and default behaviors use `CachingDisabled`. Wildcard
expansion requires route/cache review and tests. Do not add token-derived keys, cookie
partitions or an edge auth function.
Cache-enabled behaviors strip `X-Request-Id` and `X-Correlation-Id` on both hits and
misses so one viewer cannot receive another's origin IDs. Uncached behaviors retain
them. Configured CloudFront error TTLs are zero; application errors are also no-store.

### OAuth authorization-server metadata

The exact public `GET /.well-known/oauth-authorization-server` route belongs to the closed
REST route catalog and requires neither bearer nor client credentials. It uses the
CloudFront default cache-disabled behavior and origin `no-store` headers. OAuth credential
responses and protocol errors use `no-store` and `Pragma: no-cache`. The shared deployment `OAUTH_ISSUER_URL` configures both API and MCP
and selects the same closed resource policy. Discovery, protocol errors and fixed-client
setup are owned by the [OAuth contract](oauth.md).

Metadata requires no URI-bearing request input and receives no WAF exemption. OAuth
`resource` and callback URIs continue through the existing exact authorization/token path
exceptions; unrelated paths retain the counted URL-target blocks.

## MCP front door

MCP uses a separate Gateway, Lambda `live` alias, CloudFront distribution and Web ACL.
Its canonical endpoint is `https://mcp.aura-historia.com/v1`; AWS **`dev`** serves
`https://mcp.stage.aura-historia.com/v1`. The existing REST host and route catalog are
independent. [`mcp.ts`](../infra/src/constructs/mcp.ts) declares exact paths only:
`/v1` forwards methods to the native transport, including `GET`/`HEAD`/`DELETE`
rejections (`405`, `Allow: POST`); `/health` and `/ready` admit `GET`/`HEAD` (`204`).
The two exact `GET`/`HEAD` paths `/.well-known/oauth-protected-resource` and
`/.well-known/oauth-protected-resource/v1` serve the
[protected-resource metadata](mcp.md#operation-families-and-admission). No proxy,
REST handlers, sessions or SSE service are exposed on this host.

The regional custom-domain mapping has no path key or origin rewrite. CloudFront
forwards all viewer headers, query strings and cookies using AllViewer, including
`Host`, `Authorization`, `Origin`, `Accept`, `Content-Type`, `MCP-Protocol-Version`,
`Mcp-Method`, `Mcp-Name`, `Mcp-Session-Id` and `X-Correlation-Id`. Preserve response
status, `WWW-Authenticate`, `Allow`, content type and request/correlation IDs.
Every MCP behavior uses CachingDisabled, zero error TTL and a `Cache-Control:
no-store` response policy, including Gateway-generated errors and preflight. No
headers or status codes are rewritten to hide authentication/protocol errors.

Gateway owns browser preflight; its exact origin list is also passed to the native
Origin validator. Prod permits `https://aura-historia.com`; dev permits
`https://stage.aura-historia.com` and `http://localhost:3000`. There are no wildcard
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
sampling and payload logging are disabled for MCP; Gateway logs only request ID,
route key, status and integration latency. Review metrics without recording bearer
headers or arguments/results. The [hosting tests](../infra/test/mcp-hosting.test.ts)
pin these boundaries to the transport budgets.

### MCP deployment and stage verification

Use the [existing Release workflow](infra.md#routine-and-schema-dependent-releases).
Before first traffic, verify ownership and available aliases, then provision issued
ACM certificates covering the exact host in `eu-central-1` (regional Gateway) and
`us-east-1` (CloudFront viewer). Supply their ARNs through these external SSM String
parameters in the workload region:

- `/certificates/{dev|prod}/mcp-regional-certificate-arn`
- `/certificates/{dev|prod}/mcp-cloudfront-certificate-arn`

`*.aura-historia.com` does not cover `mcp.stage.aura-historia.com`. Deployment creates
the custom-domain mapping and distribution, but DNS remains operator-owned. Point
the exact public hostname's CNAME (or provider alias) to the API stack's
`McpCloudFrontDistributionDomainName` output, never to the public hostname itself or
the disabled execute-api endpoint. Keep the previous DNS target/TTL and compatible
release source for recovery. Inspect regional-origin TLS with the forwarded public
Host, the MCP `live` alias and API-scoped invoke permission before DNS cutover.

After an approved dev deployment, exercise the **public CloudFront hostname** with a
generic Streamable HTTP client. For a manual legacy protocol probe:

```sh
mcp_origin=https://mcp.stage.aura-historia.com
curl -i "$mcp_origin/ready"
curl -i "$mcp_origin/v1" \
  -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' \
  --data '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"stage-smoke","version":"1"}}}'
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
target when necessary; compatible code rollback follows the existing Release mechanism
and repoints the MCP `live` alias to an immutable version. Review diffs before rolling
back to a source predating MCP, since that may remove its front door. DNS is never
rolled back automatically, and rollback does not undo completed tool effects.

## REST client and provider configuration

| Environment | REST base URL |
| --- | --- |
| Development (AWS `dev`) | `https://api.stage.aura-historia.com/v1` |
| Production | `https://api.aura-historia.com/v1` |

Append operation suffixes exactly once, for example `/health` to the REST base URL.
Clients configured with an origin alone must include `/v1` in their request paths.
The removed `/api/v1` routes have no compatibility aliases, redirects or deprecation
period. Historical changelog entries retain their original paths.

Coordinate these externally owned settings with the backend development cutover:

- In [`aura-historia/webapp`](https://github.com/aura-historia/webapp), refresh the checked-in
  OpenAPI contract and regenerate REST/partner clients. Also update handwritten OAuth
  approval and broker token-exchange endpoints, similar-listing `Location` validation,
  partner-spec extraction, examples and test intercepts. Keep `VITE_API_URL` origin-only:
  generated operation paths already include `/v1`. Verify login, consent metadata,
  approval and code/token exchange end to end. Webapp-owned `/api/...` routes and client
  callback URLs are independent; change a callback only if it targets a renamed backend route.
- Aura discovery advertises the environment's webapp `/en/oauth/authorize` consent entry
  and the API `/v1/oauth/token` endpoint. Backend consent approval uses `/v1/oauth/authorize`;
  see the [OAuth contract](oauth.md). Do not change provider OAuth URLs.
- Update each registered WooCommerce delivery URL to the API origin plus
  `/v1/webhooks/woocommerce/{listingSourceId}` and the signed Loops preference endpoint to
  `/v1/webhooks/loops`. Preserve provider authentication, signing secrets, delivery identity
  and exact raw-body/signature handling; do not redirect deliveries. Loops' external
  `https://app.loops.so/api` base URL is unrelated and remains unchanged. Shopify/Stripe
  event ingestion is not part of this HTTP route migration.
- In [`woocommerce-extension`](https://github.com/aura-historia/woocommerce-extension),
  update the webhook URL builder, refresh its pinned backend schema/client, and trigger
  managed webhook synchronization for existing installations; changing code alone does
  not update registered delivery URLs. Keep its backend base URL origin-only. In
  [`shopify-app`](https://github.com/aura-historia/shopify-app), update the backend token
  endpoint and pinned generated client, not webapp consent-page URLs or callbacks.
- Update other project-owned scripts and test clients from OpenAPI. Preserve existing
  retry behavior: a prefix change does not make writes safe to replay.

These registrations and webapp changes are not performed by CDK or verified by local
fixtures. Local changes are not evidence that an external registration has changed.
Owners must apply them and verify requests against the development edge before
signing off the cutover; omitted live checks are not passes.

## Deployment, cutover, and rollback

Follow the [release procedure](infra.md#routine-and-schema-dependent-releases), then:

1. Inventory approved-account alias/domain claims, externally owned DNS, current
   consumers and the previous endpoint/rollback target. Resolve conflicts with their
   owners. The dev public hostname does not rename AWS stages, SSM paths or stacks.
2. Verify issued, valid ACM certificates covering the **exact public hostname**:
   regional Gateway in `eu-central-1`, CloudFront viewer in `us-east-1`. Check the
   stage references in configuration; `*.aura-historia.com` or `*.dev.aura-historia.com`
   does not cover `api.stage.aura-historia.com`. Do not infer validity from synth.
3. Review the actual diff and deployed API version/`live` alias, route catalog,
   origin Host/TLS, WAF and forwarding. Path-derived logical IDs can replace routes,
   the shared integration and its invoke permission. Check the actual Gateway route
   inventory and account quota against the create-before-delete peak, not just the
   final catalog; unmanaged routes consume that headroom too. Retain the `live`-alias
   proxy integration and API-scoped invoke permission. Probe the approved full chain:
   health/readiness, anonymous and invalid optional bearers, allowed/denied protected
   Cognito/Aura calls, OAuth, CORS, signed bodies, a valid JSON body above 16 KiB
   reaching validation, an oversized `Content-Length` returning JSON `413`, and
   cache/header isolation. Search
   readiness requires the [private-workload gate](opensearch-stage.md#dev-private-subnet-egress-and-release-gate).
   Keep credentials and signed payloads out of logs; omitted checks are not passes.
4. With approval and successful probes, update affected frontend/provider registrations
   as described [above](#rest-client-and-provider-configuration); change external DNS only
   if its target changes. Repeat probes on the public hostname, including canonical
   `/v1` reads, representative writes and signed webhooks, and verify that `/api/v1`
   operations are rejected without redirecting. Do not retain old routes or mappings.

If alias/certificate/Host/TLS/forwarding checks fail, hold cutover. After cutover,
restore the recorded DNS target and affected client/provider URLs, accounting for TTL.
[Code/stack rollback](infra.md#rollback-and-stack-recovery) does not reverse DNS or
provider effects. Preserve WAF protections and retained state. Lambda timeout
is not proof that side effects were canceled; reconcile before retrying writes.
