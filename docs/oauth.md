# Aura OAuth

Aura OAuth issues opaque Aura access tokens. Cognito authenticates the human approving
consent; its JWT is not a connector credential. Existing fixed registered clients use
exact registered callbacks, short-lived single-use authorization codes and mandatory S256
PKCE. First-party approval retains the existing default of no access-token expiry.
Delegated Aura credentials cannot approve authorization requests. The public wire contract
is in [OpenAPI](swagger.yaml).

## One logical Aura resource

REST and MCP are trusted transports of one logical Aura application resource. The
`oauth-core` resource policy owns the closed environment-specific identifiers:

| Environment | Issuer and shared Aura default | Canonical MCP identifier |
| --- | --- | --- |
| Production | `https://api.aura-historia.com` | `https://mcp.aura-historia.com/v1` |
| Stage | `https://api.stage.aura-historia.com` | `https://mcp.stage.aura-historia.com/v1` |

The composition root selects this policy with `OAUTH_ISSUER_URL`; any other issuer fails
startup. Select it together with the same environment's authoritative PostgreSQL token
store. Never share a token store or issuer configuration between production and stage.
The trusted issuer/store binding is the resource restriction for opaque credentials:
REST and MCP authenticate through the same application authentication use case, against
that local store on every request. Tokens from another environment do not exist in this
store. All existing Aura credentials, including personal and older OAuth tokens without
a resource field, already belong to this logical resource. No access-token migration or
transport-specific token family is required. Both transports must preserve scopes,
user ownership, account checks and database revocation; an MCP request never broadens a
token's authority.

## Resource indicators and grant binding

Authorize query parameters and token form bodies accept one optional `resource`
([RFC 8707](https://www.rfc-editor.org/rfc/rfc8707.html)). MCP clients must send the
canonical MCP identifier through browser consent and code exchange. Other integrations
may omit it: authorization then selects the environment's shared Aura default.
Identifiers must match one exact policy value before URL normalization. Foreign domains,
other environments, extra paths, queries, fragments and empty or repeated values fail;
they never become a default.

Each new authorization code persists its resolved identifier, including when authorization
omitted `resource`. Token exchange may repeat that exact identifier or omit `resource` to
inherit the code's binding. Substituting even the other same-environment alias fails and
consumes the code, like callback and PKCE mismatches. This binds the grant exchange;
it does not restrict the resulting credential to a transport. Foreign token resources
fail before grant consumption. Outstanding legacy codes whose persisted resource is absent
use the configured shared Aura default; invalid persisted identifiers fail closed.

Resource selection does not change requested scopes, client association, user ownership,
expiry policy or revocation. This extension introduces no refresh tokens, dynamic client
registration or client metadata discovery requirement.

## Authorization-server discovery

Both environments publish public [RFC 8414](https://www.rfc-editor.org/rfc/rfc8414.html)
metadata at `/.well-known/oauth-authorization-server` on the issuer origin. No bearer or
client credential is required or evaluated there. The response uses `Cache-Control: no-store` and `Pragma: no-cache`; the front door
keeps this exact route out of shared edge caching.

The authorization endpoint is the existing browser consent entry:
`https://aura-historia.com/en/oauth/authorize` in production and
`https://stage.aura-historia.com/en/oauth/authorize` on stage. The advertised token endpoint
is the issuer's `/v1/oauth/token`. Browser consent approves through the existing backend
`/v1/oauth/authorize`; clients begin at the advertised browser URL. Locale changes must
preserve the authorization parameters. Callback `state` behavior is unchanged and the
initial fixed-client implementation neither emits authorization-response `iss` nor claims
RFC 9207 support.

Discovery advertises authorization code, query responses, S256 PKCE and
`client_secret_post`. Its ordinary-user scope vocabulary covers profile/address/browsing
preferences (`users`), saved searches (`search-filters`), watchlist and notifications,
each with `read` and `write`. A fixed client's registered scope ceiling remains authoritative;
metadata grants no permission. Other existing registered-client scopes remain supported.
No refresh grant, `offline_access`, public-client authentication, dynamic registration or
client metadata document support is advertised. Revocation and introspection remain
available through the existing API but are omitted from discovery: revocation rejects a
token issued to another client, and introspection currently permits authenticated clients
to inspect Aura tokens beyond their own grants. Discovery does not imply broader standards
compatibility for those behaviors.

## Fixed confidential client setup

Use the existing administrator client-management API with a first-party Cognito access JWT:
`POST {issuer}/v1/admin/oauth-clients`. Supply the required client name, terms, privacy,
client and logo URLs, an exact HTTPS callback in `redirect_uris`, and the intended scope
ceiling in `scope`. For example:

```json
{
  "client_name": "Generic OAuth test client",
  "tos_uri": "https://oauth-client.example/terms",
  "policy_uri": "https://oauth-client.example/privacy",
  "client_uri": "https://oauth-client.example",
  "logo_uri": "https://oauth-client.example/logo.png",
  "redirect_uris": ["https://oauth-client.example/callback"],
  "scope": ["users:read", "watchlist:read"]
}
```

Store the returned `client_id` and one-time plaintext `client_secret` in the client's
server-side credential configuration. Register each environment separately. Scheme, host,
port, path and query must match the registered callback exactly; registering a prefix,
wildcard or the consent URL does not authorize the client's callback.

Configure any generic OAuth test client with that registration, its exact callback and the
issuer. Read discovery, generate an S256 challenge/verifier pair, and request `response_type=code`,
the chosen registered scopes, a fresh `state`, and the environment's canonical MCP `resource`.
After browser approval, validate `state` and exchange the code at the discovered token URL
using an `application/x-www-form-urlencoded` POST containing `grant_type=authorization_code`,
`code`, the original `redirect_uri`, `client_id`, `client_secret`, `code_verifier`, and the
same `resource` (or omit the resource to inherit the grant binding). Keep the client secret
out of the browser authorization URL. No branded connector account is needed.

A successful exchange retains `access_token`, `token_type: Bearer` and the space-separated
`scope`. Nonexpiring tokens omit `expires_in`; no refresh token is invented. Existing
one-time third-party exchange codes remain a separate API extension.

## Protocol errors and cache prevention

Authorization input/client/resource/scope failures and token, revocation and introspection
protocol failures return `application/json` with a standard OAuth `error` value. Malformed
or repeated ordinary parameters return `invalid_request`; unsupported grants return
`unsupported_grant_type`; invalid, expired, replayed or mismatched codes/verifiers return
`invalid_grant`; unknown or disallowed scopes return `invalid_scope`; invalid or ambiguous
resources and resource substitution return `invalid_target`. Invalid client credentials return `401 invalid_client`. Token form authentication
is exclusively `client_secret_post`; an Authorization header introduces an unsupported or
second authentication mechanism and returns `400 invalid_request`. Descriptions never reflect submitted values or
technical failure causes. See [OpenAPI](swagger.yaml) for endpoint status codes.

Credential responses, authorization redirects and protocol errors carry both
`Cache-Control: no-store` and `Pragma: no-cache`. Form bodies retain standard percent and
plus decoding; duplicate keys and invalid encoding fail before values enter a map. Client
management, consent metadata and first-party authentication/authority errors retain the
existing REST problem vocabulary. No unvalidated callback is used for an error redirect.
The [front-door contract](http-api-front-door.md) owns exact routing and URI-bearing WAF
exceptions.
