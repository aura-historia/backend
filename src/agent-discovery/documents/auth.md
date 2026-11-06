# auth.md: Aura Historia

How agents authenticate with Aura Historia ({{site_origin}}), a discovery service for art,
antiques and design objects offered by dealers, auction houses, marketplaces and auction
platforms. The public catalogue is open to everyone. A person's own data needs their approval.

## Anonymous access

Searching and reading the public catalogue needs no account, registration or token:

- **MCP:** `{{mcp_endpoint}}` with the Streamable HTTP transport.
- **REST:** `{{api_origin}}/v1`, described by the OpenAPI document `{{documentation_origin}}/swagger.yaml`.

Send no `Authorization` header for anonymous calls. A supplied token is always checked, and an
invalid one is rejected even on a public call.

## Signing a user in

A user's profile and addresses, saved searches, watchlist and notifications need the user's
approval. Aura Historia runs its own OAuth authorization server; REST and MCP accept the same
tokens.

| Item | Value |
| --- | --- |
| Issuer | `{{issuer}}` |
| Authorization server metadata | `{{issuer}}/.well-known/oauth-authorization-server` |
| MCP protected-resource metadata | `{{mcp_origin}}/.well-known/oauth-protected-resource/v1` |
| REST protected-resource metadata | `{{api_origin}}/.well-known/oauth-protected-resource` |
| User scopes | {{ordinary_user_scopes}} |

Every sign-in uses the authorization code grant with S256 PKCE. The user approves the request
on the Aura Historia website. Every authorization response carries `iss`; reject a response
whose `iss` is not `{{issuer}}`. A `401` from a protected operation points to its
protected-resource metadata, and on MCP also names the scopes the operation needs.

### Agents and other public clients

Agents sign users in without registering, with a Client ID Metadata Document. Use this flow
when the authorization server metadata has `client_id_metadata_document_supported: true`.

- `client_id` is the HTTPS URL of your Client ID Metadata Document. Aura Historia fetches it
  at sign-in. It must contain `client_id` equal to its own URL, `client_name`, your
  `redirect_uris`, `token_endpoint_auth_method: none` and `grant_types` with
  `authorization_code`, and no client secret.
- Callbacks are HTTPS URLs, or HTTP URLs on `127.0.0.1`, `[::1]` or `localhost`, where any
  port matches.
- Request only user scopes, and send `resource={{mcp_endpoint}}` for MCP.
- Exchange the code at the token endpoint with `client_id` and `code_verifier`, without a
  secret.
- Tokens are short-lived; the token response states the lifetime in `expires_in`. Sign the
  user in again when a token is rejected.

There is no dynamic client registration.

### Registered clients

Integrations that need more than the user scopes, such as shop platforms that synchronize a
partner's listings, use a confidential client that Aura Historia's administrators register for
them. Its `client_id` starts with `oc_`. It authenticates at the token endpoint with
`client_secret_post` and uses exact, registered HTTPS callbacks.

## Personal access tokens

A signed-in user can create a personal access token with the scopes and expiry they choose in
the access-token section of their Aura Historia dashboard, for example for their own scripts
or a partner's listing synchronization. The token is shown once. Send it as
`Authorization: Bearer <token>`, keep it in secret storage, and never put it in a URL.

## Disconnecting

The access-token section of the dashboard lists personal access tokens and every connected
agent or integration. Revoking an entry there makes the next request with its token fail.
