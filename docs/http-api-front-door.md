# HTTP API front door

This document owns edge routing, authentication, forwarding and cache/security rules.
[OpenAPI](swagger.yaml) owns request/response contracts; [ProductListing](product-listing.md)
and [marketing consent](marketing-consent.md) own use-case semantics. Source/synth do not
verify live DNS, certificates, aliases, WAF or forwarding; operators must inspect them.

## Route and authentication boundary

[`api.ts`](../infra/src/constructs/api.ts) owns a closed Gateway route catalog, checked
against OpenAPI and Axum by [`api-route-matrix.test.ts`](../infra/test/api-route-matrix.test.ts).
There is no proxy or `$default` route. Ordinary REST methods target the `live` alias
of the single stage API Lambda; deploy its version/alias before the API stack uses it.

Gateway deliberately applies **no Cognito JWT authorizer**. Axum validates Cognito
access JWTs and Aura opaque access tokens, optional bearers, delegated scopes and
required user/admin/partner authorization. Anonymous probes remain anonymous;
optional-auth routes reject invalid supplied credentials. OAuth and signed-provider
handlers retain their dedicated credential/proof checks. A Cognito-only edge policy
would reject valid application credentials before those handlers can evaluate them.

Preserve request headers, cookies, query strings and exact signed bodies through the
Lambda proxy integration, including JSON bodies on DELETE. Do not rewrite webhook
payloads or move one-time confirmation into GET/redirect processing. Gateway preflight
and Axum response CORS must retain approved origins, required authorization/provider
headers and `Idempotency-Key` admission/exposure. CORS is not authorization; do not
relax raw-body limits or application checks to fix an edge integration.

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
  rules; `NoUserAgent_HEADER` is count-only. There is no additional WAF rate rule.
  Gateway stage throttles shape requests, not database or Lambda concurrency/capacity.

## Selective anonymous discovery caching

Only explicitly approved anonymous `200` discovery representations may be shared.
Any `Authorization` header means `private, no-store`, including malformed credentials.
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

Approved responses use `public, max-age=0, s-maxage=<TTL>, stale-if-error=0`.
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
   Cognito/Aura calls, OAuth, CORS, signed bodies and cache/header isolation. Search
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
