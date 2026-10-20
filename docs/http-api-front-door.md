# HTTP API front door

This document owns edge routing, authentication, forwarding and cache/security rules.
[OpenAPI](swagger.yaml) owns request/response contracts. [ProductListing](product-listing.md),
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
Ordinary requests make no live Cognito status call; signing keys retain their existing
bounded refresh behavior. No additional Cognito scope is required.

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
and Axum response CORS must retain approved origins, required authorization/provider
headers and `Idempotency-Key` admission/exposure. CORS is not authorization; do not
relax raw-body limits or application checks to fix an edge integration. Header forwarding
does not opt an operation into replay; preserve the [keyless creation exceptions](command-retries.md).

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
- Advertise Aura's authorization endpoint at `/v1/oauth/authorize` and token endpoint at
  `/v1/oauth/token`, using the environment's API origin. Do not change provider OAuth URLs.
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
