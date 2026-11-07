---
name: connect-account
description: Sign a person in to their Aura Historia account from an agent so it can read and manage their watchlist, saved searches, notifications and profile, keep the connection working and disconnect it again. Use when a task needs the user's own Aura Historia data rather than the public catalogue.
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
   registration step. The document declares `token_endpoint_auth_method: none`, `grant_types`
   with `authorization_code` and `refresh_token`, and your `redirect_uris`; a loopback callback
   on `127.0.0.1`, `[::1]` or `localhost` matches on any port.
3. Send the user to the advertised `authorization_endpoint` with `response_type=code`, your
   `client_id` and `redirect_uri`, an S256 PKCE challenge, a fresh `state`, the scopes you need
   and `resource={{mcp_endpoint}}`. The user signs in and approves the request on the Aura
   Historia website.
4. At the callback, check `state` and that `iss` equals `{{issuer}}`. If the user declined,
   you receive `error=access_denied`; do not retry without asking them.
5. Exchange the code at the advertised `token_endpoint` with `grant_type=authorization_code`,
   the `code`, your `client_id`, the `code_verifier` and the same `redirect_uri`, and no
   client secret. The response holds an access token, its lifetime in `expires_in` and a
   refresh token.
6. Send the access token as `Authorization: Bearer <token>` on MCP or REST requests.

Ask only for the scopes the task needs: {{ordinary_user_scopes}}. A write scope does not
include its read scope. When a `403` with `error="insufficient_scope"` asks for more, run a
new authorization that requests the scopes you already hold together with the new ones.

## Keep the connection working

Access tokens are short-lived. When one expires, a request returns `401` with
`error="invalid_token"` in its Bearer challenge. Refresh it at the token endpoint with `grant_type=refresh_token`,
the `refresh_token` and your `client_id`, without a secret.

- Every refresh returns a new refresh token and the old one stops working at once. Store the
  new one before using the new access token.
- Refresh one token at a time. Presenting a refresh token that was already replaced signs
  that installation out and revokes the connection's access tokens.
- `invalid_grant` means the refresh token can no longer be used, for example because it
  expired or the user disconnected: sign the user in again.

Never put tokens or verifiers in URLs, logs or messages to the user.

## Disconnect

The user sees each connected agent as one entry in the access-token section of their Aura
Historia dashboard and can disconnect it there; the next request with its tokens is rejected.
To sign out yourself, revoke your refresh token at `{{issuer}}/v1/oauth/revoke` with `token`
and your `client_id`, without a secret. That ends the whole connection, including other
installations of your client for the same user.
