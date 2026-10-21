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
