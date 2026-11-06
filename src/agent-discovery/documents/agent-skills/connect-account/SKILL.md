---
name: connect-account
description: Sign a person in to their Aura Historia account from an agent so it can read and manage their watchlist, saved searches, notifications and profile, and explain how they disconnect it again. Use when a task needs the user's own Aura Historia data rather than the public catalogue.
---

# Connect a user's Aura Historia account

The public catalogue needs no account. Sign the user in only for their own data: profile and
addresses, saved searches, watchlist and notifications. The full sign-in contract is
`{{auth_md_url}}`.

## Sign in

Aura Historia runs its own OAuth authorization server. Its issuer is `{{issuer}}`, and its
metadata is at `{{issuer}}/.well-known/oauth-authorization-server`.

1. Call a protected MCP tool on `{{mcp_endpoint}}` without a token, or read
   `{{mcp_origin}}/.well-known/oauth-protected-resource/v1`. The `401` challenge names that
   metadata and the scopes the tool needs; the metadata names the authorization server.
2. When the authorization server metadata has `client_id_metadata_document_supported: true`,
   use the HTTPS URL of your Client ID Metadata Document as `client_id`. There is no
   registration step. The document declares `token_endpoint_auth_method: none` and your
   callbacks; a loopback callback on `127.0.0.1`, `[::1]` or `localhost` matches on any port.
3. Send the user to the advertised `authorization_endpoint` with `response_type=code`, an S256
   PKCE challenge, a fresh `state`, the scopes you need and `resource={{mcp_endpoint}}`. The
   user signs in and approves the request on the Aura Historia website.
4. At the callback, check `state` and that `iss` equals `{{issuer}}`. If the user declined,
   you receive `error=access_denied`; do not retry without asking them.
5. Exchange the code at the advertised `token_endpoint` with `client_id`, the `code_verifier`
   and the same `redirect_uri`, and no client secret.
6. Send the access token as `Authorization: Bearer <token>` on MCP or REST requests.

Ask only for the scopes the task needs: {{ordinary_user_scopes}}. A write scope does not
include its read scope. To add scopes later, run a new authorization.

## Keep the connection working

The token response states how long the token lives (`expires_in`). When a request with the
token returns `401` (on MCP with `error="invalid_token"`), the token has expired or the user
has revoked it: sign the user in again. A `403` with `error="insufficient_scope"` lists the
scopes the operation needs. Never put tokens or verifiers in URLs, logs or messages to the
user.

## Disconnect

The user sees every connected agent in the access-token section of their Aura Historia
dashboard and can disconnect it there; the next request with that token is rejected. An agent
that should stop acting for the user deletes its stored tokens.
