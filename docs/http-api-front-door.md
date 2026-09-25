# HTTP API front door

This document records the checked-in target for the HTTP API migration. It was prepared from approved `develop` baseline `eb3a7844fc110c691937eea6820d35b74f98467d`. It is an implementation and rollout guide, not evidence of live DNS, certificate, alias, WAF, or forwarding configuration. Operators MUST verify those values in the approved environment inventory before deployment or cutover.

## Route and authentication boundary

`infra/src/constructs/api.ts` owns the closed Gateway route matrix. `infra/test/api-route-matrix.test.ts` checks every matrix operation against both Axum route declarations and `swagger.yaml`; no proxy or `$default` route is configured. All ordinary REST methods use the `live` alias of the one `aura-historia-api-<stage>` Lambda.

Gateway deliberately applies no Cognito JWT authorizer to these routes. Axum remains the authentication boundary because the public contract accepts both Cognito access JWTs and Aura opaque access tokens, has optional authentication on discovery reads, and contains OAuth and signed WooCommerce intake flows. Consequently:

- anonymous health/readiness probes stay anonymous;
- optional-bearer discovery and newsletter routes can run anonymously but reject invalid supplied credentials;
- user, administrator, and partner routes enforce their required application bearer and authorization checks in Axum;
- OAuth credentials and WooCommerce raw-body/signature validation reach their dedicated Axum handlers unchanged.

This is not an authorization bypass: every protected handler retains its existing application authorization. It prevents a Cognito-only Gateway policy from incorrectly rejecting valid non-Cognito credentials before the application can evaluate them.

## Front-door controls

| Control | Decision | Security and cache implication |
| --- | --- | --- |
| HTTP API custom domain | Retain regional TLS 1.2 domain and default API mapping. | `/api/v1` is forwarded unchanged; `AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH=true` keeps the named stage out of Axum paths. DNS records are externally owned and must be verified before cutover. |
| CloudFront | Retain for configured dev/prod aliases and HTTPS redirect. | The origin forwards viewer headers, query strings, and cookies. API caching is disabled for `/api/*` as well as the default behavior, so optional/personalized responses cannot share an edge cache key. The former JWT-only cache-key function is omitted because it did not cover opaque Aura tokens. |
| WAF | Retain the CloudFront-scoped AWS IP reputation, common-rule-set, and known-bad-input managed rules. | The common rule set's `NoUserAgent_HEADER` remains count-only. There is no new WAF rate rule in this migration. |
| CORS | Retain HTTP API preflight and Axum response CORS. | Gateway permits `Authorization`, `Content-Type`, `Accept`, `X-Correlation-Id`, and required WooCommerce headers. Axum retains response correlation headers and raw-body limits. |
| Throttling | Retain the explicit stage policy: prod burst/rate `5000/2000`, dev/ephemeral `50/20`. | This is Gateway request shaping, not a database connection, account-Lambda concurrency, or hard capacity guarantee. No reserved/provisioned concurrency or hidden replacement cap is introduced. Route-family limits remain absent until a separately reviewed policy specifies values and WAF/application ownership. |

CloudFront, WAF, API Gateway access logging, and Lambda invocations all incur their normal request/logging charges. Disabling API edge caching can increase origin/Lambda request volume; it is intentional until a response-by-response cache policy demonstrates that shared caching is safe.

## Deployment, cutover, and rollback

1. Build artifacts and synthesize the API/compute stacks. Review the CloudFormation diff: one API alias, one ordinary Lambda integration, explicit route resources, retained custom domain/CloudFront/WAF, and no changes to EventBridge, SQS, Cognito triggers, or legacy special-intake integrations.
2. Before deployment, verify the actual regional and CloudFront ACM certificates, DNS ownership, dev wildcard aliases, production alias, WAF ACL, CloudFront behaviors, and any temporary forwarders retained under #449. Do not infer live state from this repository.
3. Deploy the API Lambda version and `live` alias before (or atomically with) the API stack that references it. The compute stack's `CommitSHA` parameter selects the artifact and replaces the immutable API version via its description; `live` follows that version. A code-only release or parameter rollback must update both the function code and the alias target, while an unchanged parameter/configuration must not force a new version. Run sanitized allowed and denied smoke requests through the approved front-door chain: anonymous/optional discovery, required/admin bearer, Aura opaque bearer, OAuth, preflight/correlation headers, cookies/query forwarding, and signed WooCommerce body handling. Never place tokens or signed payloads in logs.
4. Only after the smoke results and reviewed diff are approved may DNS or production traffic be changed. Keep legacy integrations until their replacement owner and coverage proof are recorded.

To roll back, first move the `live` alias to the last known-good API Lambda version, then roll back the API stack route/domain change if necessary. Retain the custom domain, CloudFront distribution, WAF ACL, legacy integrations, and provider/Cognito/event resources during rollback. A Lambda timeout does not cancel in-flight database or provider effects; operators must use the owning service's reconciliation procedure before retrying side-effecting requests.
