# Aura OAuth

Aura OAuth issues opaque Aura access tokens. Cognito authenticates the human approving
consent; its JWT is not a connector credential. Two kinds of client sign users in, told
apart by the submitted `client_id`, and neither ever falls back to the other:

- **Registered clients** (`oc_…`) are confidential clients an administrator creates. They
  authenticate with `client_secret_post`, use exact HTTPS callbacks and receive the
  first-party default of non-expiring tokens.
- **Metadata document clients** are public clients whose `client_id` is the HTTPS URL of
  their Client ID Metadata Document. They hold short-lived access tokens and rotating
  refresh tokens, grouped into one connection per user. See
  [metadata document clients](#metadata-document-clients).

Both use short-lived single-use authorization codes and mandatory S256 PKCE. Delegated Aura
credentials cannot approve authorization requests. The public wire contract is in
[OpenAPI](swagger.yaml).

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
expiry policy or revocation. A refresh-token family keeps the resource of the code that
started it; a refresh may repeat it or omit `resource`, and any other identifier fails with
`invalid_target` without consuming the refresh token.

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
preserve the authorization parameters. Callback `state` behavior is unchanged.

Every authorization response carries the issuer as `iss` ([RFC 9207](https://www.rfc-editor.org/rfc/rfc9207.html)),
for every client kind, and discovery advertises `authorization_response_iss_parameter_supported`.
The backend adds `iss` to each code redirect it returns. The consent page, which sends the
`access_denied` redirect when the user declines, must add the same `iss` and may send it
only to a callback the backend validated for that client: the registered list, or the
callback the [consent metadata](#consent-metadata) accepted.

Discovery advertises authorization code, query responses, S256 PKCE and
`client_secret_post`. Its ordinary-user scope vocabulary covers profile/address/browsing
preferences (`users`), saved searches (`search-filters`), watchlist and notifications,
each with `read` and `write`. A fixed client's registered scope ceiling remains authoritative;
metadata grants no permission. Other existing registered-client scopes remain supported.
While metadata document clients are enabled, discovery also advertises
`client_id_metadata_document_supported: true` and adds `none` to
`token_endpoint_auth_methods_supported`; clients such as Claude use metadata documents only
when both are present. It then also adds `refresh_token` to `grant_types_supported` and
`offline_access` to `scopes_supported`; the MCP protected-resource metadata never lists
`offline_access`. Dynamic registration is not supported. Revocation and introspection remain
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

Nonexpiring grants stay usable until the owner revokes them, the user is suspended, or the
authoritative grant changes; they do not require periodic refresh or reauthentication.
This makes secure server-side storage and a discoverable disconnect control essential.
Use Aura's existing account token listing/revocation controls to inspect scope/expiry and
disconnect an individual connection without displaying its bearer. Revocation rejects the
next REST or MCP authorization check. It does not retroactively cancel work already
authorized. After revocation, reconnect through a new human authorization and code exchange;
the old bearer cannot be refreshed. A deliberately expiring token is rejected after expiry.
See [generic MCP acceptance and stage verification](mcp.md#generic-oauth-acceptance-and-stage-smoke)
for local coverage and the optional bounded stage command.

## Metadata document clients

A `client_id` that parses as an `https` URL identifies a public client by its Client ID
Metadata Document ([MCP authorization 2026-07-28](https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization),
[draft-ietf-oauth-client-id-metadata-document](https://datatracker.ietf.org/doc/html/draft-ietf-oauth-client-id-metadata-document-00)).
There is no registration call and no stored client row; Aura fetches the document from the
`client_id` URL. Only the URL's canonical serialization is accepted: a domain host, the
default port, a path below the root, and no credentials or fragment. `client_id` values
therefore compare as exact strings.

**Trust policy.** Aura accepts any HTTPS `client_id` whose document validates, so agents
sign in without an onboarding step. The consent page shows the user the verified client
host and callback host, which a document cannot fake, beside the document's self-asserted
name and logo. Operators stop abusive clients with a host denylist: an entry denies the
domain and its subdomains, before any fetch and again at the token endpoint. Per
environment, `OAUTH_METADATA_DOCUMENT_CLIENTS_ENABLED` (default `false`) turns the whole
client kind on, and `OAUTH_METADATA_DOCUMENT_DENIED_HOSTS` lists the denied domains,
comma-separated; [`infra/src/config.ts`](../infra/src/config.ts) sets both, and enables the
kind in every environment. While disabled, URL client IDs are rejected and discovery does
not advertise them. A public client must never hold a non-expiring token, so enabling the
kind requires the [refresh tokens](#refresh-tokens) below.

**Outbound policy.** Document fetches fail closed: HTTPS on port 443 only, to a host whose
every resolved address is public under the shared public-network policy, with the
connection pinned to those addresses. Redirects, responses other than `200`, content types
other than `application/json`, bodies above 5 KiB and fetches exceeding 3 seconds fail.

**Caching.** A fetched document is reused for its `Cache-Control: max-age` or `Expires`
lifetime, raised to at least 5 minutes and capped at 1 hour, per API process. After that
the document is fetched again, and a failed fetch fails the request; a stale document is
never served.

**Validation.** The document must contain `client_id` equal to the document URL,
`client_name` (at most 128 characters), at least one `redirect_uris` entry,
`token_endpoint_auth_method: none` and `grant_types` containing `authorization_code`. It must
not declare a client secret. `response_types`, if present, must contain `code`;
`client_uri` and `logo_uri`, if present, must be HTTPS URLs. Callbacks must be HTTPS, or
plain HTTP on the loopback hosts `127.0.0.1`, `[::1]` and `localhost`. An unavailable or
invalid document makes the client invalid; no error is ever sent to a callback at that
point.

**Authorization.** A requested callback must match a listed one exactly, except that a
listed loopback callback matches on any port ([RFC 8252 §7.3](https://www.rfc-editor.org/rfc/rfc8252.html#section-7.3)),
because native clients bind an ephemeral port; scheme, host, path and query still match
exactly. Requested scopes must lie within the ordinary-user vocabulary that discovery
advertises. Approval records the [connection](#connections) and the authorization code
persists the `client_id` URL.

**Token exchange.** The client sends `client_id` without a secret; a `client_secret` from
it is `invalid_client`. S256 PKCE and the resource rules above apply unchanged, and only the
`client_id` the code was issued to may exchange it. A rejected exchange consumes the code,
except when a metadata document client presents another client's code: having proved no
credential, it cannot burn that code. A code whose connection the user ended in the
meantime fails with `invalid_grant`. The client receives an expiring Bearer token within its
connection and no third-party exchange code, and a refresh token when the
[issuance rule](#refresh-tokens) applies. An individual access token's scopes and expiry can
only be narrowed afterwards, not widened or removed.

### Connections

A connection is one user's grant to one metadata document client. It holds the union of
the scopes the user approved, when it was created and last used, the client's access
tokens and the refresh-token families of its sign-ins. Every approval adds its scopes to the
connection and refreshes the client name the document asserts; MCP clients asking for more
scopes request the union of earlier and newly challenged scopes, and the
[consent metadata](#consent-metadata) shows which scopes are already granted. The
connection is last used when the user approves the client or the client obtains an access
token, by code exchange or refresh.

Each completed sign-in starts its own refresh-token family. Several installations of the
same client, such as Claude Code on two machines, therefore stay signed in side by side,
and a refresh issues the scopes of its own sign-in. A connection keeps at most five
families: a further sign-in ends the least recently used one, which bounds the families a
re-signed-in installation leaves behind. Ending the connection, by the user or by the client
revoking a refresh token, ends every family and access token it holds, so one installation
signing out signs out all of them. Reuse detection likewise revokes the access tokens of all
installations, which then refresh with their own families. Disconnecting also withdraws
approvals the client has not exchanged yet.

Users see each connection as one entry of the account's
[access-token listing](swagger.yaml), not its rotating access tokens, and end it with
`DELETE /v1/me/access-tokens?clientId=…`. That call revokes all of the user's credentials
for one client: the connection of a metadata document client, or every access token of a
registered client. Like issuing an access token, it requires a first-party Cognito session;
a delegated Aura credential receives `403`. An administrator revoking all of a user's access
tokens also ends all of the user's connections.

### Token lifetimes

| Client kind | Access token | Refresh token |
| --- | --- | --- |
| Registered client | Does not expire | None |
| Metadata document client | 1 hour | Rotating; ends 30 days after its last use and at the latest 90 days after sign-in |
| Personal access token | As the user chooses | None |

`OAUTH_METADATA_DOCUMENT_ACCESS_TOKEN_TTL_SECONDS`, `OAUTH_REFRESH_TOKEN_IDLE_TTL_SECONDS`
and `OAUTH_REFRESH_TOKEN_ABSOLUTE_TTL_SECONDS` configure the metadata document client
lifetimes per environment; the idle limit may not exceed the absolute limit. An expired
access token receives `401` on REST and MCP, whose challenge reports `invalid_token`, which
makes the client refresh.

### Refresh tokens

**Issuance.** The code exchange returns a `refresh_token` when the client's document lists
`refresh_token` in `grant_types`, or when the authorization request's `scope` included
`offline_access`. `offline_access` is a request flag, not an Aura scope: it never appears on
a token. A registered client requesting it receives `invalid_scope`.

**Refresh.** A metadata document client sends `grant_type=refresh_token`, `refresh_token`
and its `client_id`, without a secret, to the token endpoint. `scope` may narrow the new
access token to a subset of its sign-in's scopes; `resource` may repeat the sign-in's
resource. Registered clients receive `unauthorized_client`.

**Rotation.** Every successful refresh returns a new refresh token and a new access token.
The presented refresh token stops working at once.

**Reuse detection.** Aura keeps every replaced token of a family until the family ends.
Presenting one again means that someone else may hold the family: Aura ends the family and
revokes every access token of the connection. Other families of the connection keep their
refresh tokens.

**Concurrent refreshes.** Refreshes of one family are serialized. When two requests present
the same token, the first rotates it and the second fails with `invalid_grant`. A token
replaced at most 10 seconds earlier is treated as such a race, not as reuse, and leaves the
family intact. After that, presenting it is reuse.

**Expiry.** A family ends when its current token was not used for the idle limit, and at the
latest at its absolute limit, counted from the sign-in. Expiry is checked on every refresh.

**Errors.** Every unusable refresh token returns `400 invalid_grant`, whatever the reason:
unknown, malformed, expired, replaced or reused, issued to another client, or of an ended
connection. A `scope` beyond the sign-in's returns `invalid_scope`, another resource
`invalid_target`, and a disabled or denied client `401 invalid_client`; none of them consumes
the token. A refresh needs no outbound call and answers within the token endpoint's
ordinary database budget.

**Revocation.** `POST /v1/oauth/revoke` accepts a metadata document client identified by its
`client_id` without a secret. It may revoke its own access tokens, which ends only that
token, and its own refresh tokens, which ends the whole connection. Another client's token
is refused, and an unknown token succeeds as already revoked.

**Cleanup.** The scheduled credential cleanup removes the tokens of expired families, then
the families, then connections that hold no access token, no family and no outstanding
authorization code, each in bounded batches.

### Consent metadata

`GET /v1/oauth/metadata-document-clients?client_id=…&redirect_uri=…` serves the consent page for a metadata
document client with a first-party Cognito JWT. It takes the URL client ID as a query
parameter, because a URL does not fit the `/v1/oauth/clients/{clientId}` path that registered
clients keep using. It loads and validates the current document and returns `400` unless
the callback matches it. The response carries the document's name and optional client and
logo URIs, the `client_id` host, the callback host, whether the document lists only loopback
callbacks, the scope ceiling and the scopes the user already granted the client.

## Protocol errors and cache prevention

Authorization input/client/resource/scope failures and token, revocation and introspection
protocol failures return `application/json` with a standard OAuth `error` value. Malformed
or repeated ordinary parameters return `invalid_request`; unsupported grants return
`unsupported_grant_type`, and a grant the client may not use `unauthorized_client`; invalid,
expired, replayed or mismatched codes, verifiers and refresh tokens return `invalid_grant`; unknown or disallowed scopes return `invalid_scope`; invalid or ambiguous
resources and resource substitution return `invalid_target`. Invalid client credentials,
and a metadata document client that is disabled, denied, unavailable or invalid, return
`invalid_client`: `401` at the token endpoint, `400` at the authorization endpoint, whose
caller is the authenticated consent page. Token form authentication is `client_secret_post`
for registered clients and `none` for metadata document clients; an Authorization header
introduces an unsupported or second authentication mechanism and returns
`400 invalid_request`. Descriptions never reflect submitted values or technical failure
causes. See [OpenAPI](swagger.yaml) for endpoint status codes.

Credential responses, authorization redirects and protocol errors carry both
`Cache-Control: no-store` and `Pragma: no-cache`. Form bodies retain standard percent and
plus decoding; duplicate keys and invalid encoding fail before values enter a map. Client
management, consent metadata and first-party authentication/authority errors retain the
existing REST problem vocabulary; an unusable metadata document there is
`400 OAUTH_INVALID_CLIENT`. No unvalidated callback is used for an error redirect.
The [front-door contract](http-api-front-door.md) owns exact routing and URI-bearing WAF
exceptions.
