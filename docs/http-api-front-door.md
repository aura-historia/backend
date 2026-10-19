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
- Regional API Gateway uses TLS 1.2 and a default custom-domain mapping. The public
  `/api/v1` path stays unchanged; the Lambda adapter normalizes a matching Gateway
  stage prefix. Keep `AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH=true`.
- CloudFront redirects viewers to HTTPS and uses an HTTPS-only regional Gateway
  origin (`attrRegionalDomainName`), **not the public CloudFront-facing DNS record**.
  AllViewer forwards `Host`; origin TLS and custom-domain mapping must accept it.
  The default execute-api endpoint is disabled when the custom domain is configured.
- Retain CloudFront-scoped AWS IP reputation, common-rule-set and known-bad-input
  rules; `NoUserAgent_HEADER` and `SizeRestrictions_BODY` are count-only. There is no
  additional WAF rate rule. Gateway stage throttles shape requests, not database or
  Lambda concurrency/capacity.
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

Approved responses use `public, max-age=0, s-maxage=<TTL>, stale-if-error=0`. Listing
responses containing advertised shipping cap this TTL at referenced geographic provider
evidence retention deadlines; shipping observation age does not expire terms. Destination
personalization remains private and uncached.
The custom policy has zero minimum/default TTL, keys on `Authorization`, `Origin`,
`Host` and **all query strings**, and enables gzip/Brotli variation. Cookies are
forwarded but not keyed: never add cookie-dependent data to a shared representation.
OPTIONS is not cached; no stale-while-revalidate or positive stale-on-error window exists.

Only reviewed discovery path behaviors use this policy; generic `/api/*` and default
behaviors use `CachingDisabled`. Wildcard expansion requires route/cache review and
tests. Do not add token-derived keys, cookie partitions or an edge auth function.
Cache-enabled behaviors strip `X-Request-Id` and `X-Correlation-Id` on both hits and
misses so one viewer cannot receive another's origin IDs. Uncached behaviors retain
them. Configured CloudFront error TTLs are zero; application errors are also no-store.

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
   origin Host/TLS, WAF and forwarding. Probe the approved full chain before DNS:
   health/readiness, anonymous and invalid optional bearers, allowed/denied protected
   Cognito/Aura calls, OAuth, CORS, signed bodies, a valid JSON body above 16 KiB
   reaching validation, an oversized `Content-Length` returning JSON `413`, and
   cache/header isolation. Search
   readiness requires the [private-workload gate](opensearch-stage.md#dev-private-subnet-egress-and-release-gate).
   Keep credentials and signed payloads out of logs; omitted checks are not passes.
4. With approval and successful probes, move external DNS and affected frontend/provider
   registrations; repeat probes on the public hostname. Keep the old endpoint until
   its consumers are migrated and verified. Redirecting authenticated requests or
   signed webhooks is not a safe default, and old-host retirement is not automatic.

If alias/certificate/Host/TLS/forwarding checks fail, hold cutover. After cutover,
restore the recorded DNS target and affected client/provider URLs, accounting for TTL.
[Code/stack rollback](infra.md#rollback-and-stack-recovery) does not reverse DNS or
provider effects. Preserve the old front door, WAF and retained state. Lambda timeout
is not proof that side effects were canceled; reconcile before retrying writes.
