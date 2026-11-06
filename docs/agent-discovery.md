# Agent discovery

The backend writes every machine-readable document that describes Aura to agents. The API and
MCP hosts serve the documents about themselves; the webapp republishes selected documents on
the website origin, but never writes them. Each document is built from the code and
configuration of the service it describes, so it cannot drift from what that service runs.

This document owns the discovery contract. [OAuth](oauth.md#authorization-server-discovery)
owns authorization-server metadata, [MCP](mcp.md#operation-families-and-admission) owns the MCP
protected-resource metadata and challenges, and the [front door](http-api-front-door.md) owns
edge routing, caching and stage verification.

## Environment

Every URL comes from the `oauth-core` resource policy that `OAUTH_ISSUER_URL` selects. Any other
issuer fails startup, so a document can never mix environments.

| Origin | Production | Stage |
| --- | --- | --- |
| Website | `https://aura-historia.com` | `https://stage.aura-historia.com` |
| API, issuer and shared Aura default resource | `https://api.aura-historia.com` | `https://api.stage.aura-historia.com` |
| MCP host (resource: its `/v1` endpoint) | `https://mcp.aura-historia.com` | `https://mcp.stage.aura-historia.com` |
| Documentation | `https://docs.api.aura-historia.com` | `https://docs.api.aura-historia.com` |

Tests render each document for both environments and fail if any URL lies outside that
environment's origins. The only exception is a document's schema identifier, such as the
server card's `$schema`.

## Documents

| Document | Host and path | Media type | Caching |
| --- | --- | --- | --- |
| [MCP Server Card](#mcp-server-card) | MCP `GET`/`HEAD /.well-known/mcp/server-card.json` | `application/mcp-server-card+json` | `public, max-age=300` |
| [API catalog](#api-catalog) | API `GET`/`HEAD /.well-known/api-catalog` | `application/linkset+json; profile="https://www.rfc-editor.org/info/rfc9727"` | Shared, 300 s |
| [REST protected-resource metadata](#rest-protected-resource-metadata) | API `GET /.well-known/oauth-protected-resource` | `application/json` | `no-store` |

Discovery documents are public, read-only and identical for every viewer. They evaluate no
credentials or query strings and allow any origin (`Access-Control-Allow-Origin: *`). Only a
`200` may be cached; errors stay `no-store`. OAuth metadata, on either host, is always
`no-store`.

### MCP Server Card

The card follows the merged [SEP-2127](https://modelcontextprotocol.io/seps/2127-mcp-server-cards)
format and its [schema](https://static.modelcontextprotocol.io/schemas/v1/server-card.schema.json).
The MCP router serializes it once, from the same handler that answers `initialize` and
`server/discover`:

- `title`, `version`, `websiteUrl` and `icons` are that handler's `serverInfo`, so the card
  reports the deployed release.
- `name` is the reverse-DNS `com.aura-historia/mcp` that the card format and registries
  require; `serverInfo.name` keeps the stable programmatic identifier `aura-historia-mcp`.
  `description` is a summary within the card's 100-character limit; directories display the
  longer `serverInfo` description.
- One `streamable-http` remote is the canonical MCP endpoint, with exactly the protocol
  versions the transport supports.
- `_meta["com.aura-historia/oauth-protected-resource"]` is the path-suffixed MCP
  protected-resource metadata URL that protected-call challenges also name.

Tests validate the card against the SEP-2127 schema. The card lists no tools and no
capabilities: SEP-2127 excludes
both, because they can vary per client and identity. Clients learn them from `server/discover`
or `initialize`, whose live values win if they ever disagree with the card.

SEP-2127 recommends `<endpoint>/server-card` and leaves the location free when a catalog links
to it. Aura serves the card at the path that the website's AI catalog links to and that agent
readiness checks probe. The route keeps the MCP Host allowlist but not its Origin allowlist.

### API catalog

The catalog follows [RFC 9727](https://www.rfc-editor.org/rfc/rfc9727) in the
[Linkset](https://www.rfc-editor.org/rfc/rfc9264) format. Its first context is anchored at the
catalog's own URL and lists both APIs as `item` links. Each API then has its own context:

| `anchor` | `service-desc` | `service-doc` | `status` |
| --- | --- | --- | --- |
| API `/v1` | Documentation `/swagger.yaml` (OpenAPI) | Documentation site | API `/v1/health` |
| MCP `/v1` | The MCP Server Card | Documentation `/mcp.md` | MCP `/ready` |

`GET` and `HEAD` both carry `Link: <catalog URL>; rel="api-catalog"`, which RFC 9727 requires on
`HEAD`. The catalog is an approved anonymous discovery representation with a 300-second shared
TTL; a request carrying `Authorization` is answered with `private, no-store`.

### REST protected-resource metadata

The API host serves [RFC 9728](https://www.rfc-editor.org/rfc/rfc9728) metadata for the shared
Aura default resource: `resource` and the single `authorization_servers` entry are the issuer,
`bearer_methods_supported` is `header`, and `scopes_supported` is the ordinary-user vocabulary
that authorization-server discovery also advertises. A client's scope ceiling remains
authoritative. `resource` is the shared default, the API origin, not each request URL: a client
that follows a challenge here must accept a `resource` that is the origin of the URL it
requested, as MCP clients already do. RFC 9728 §3.3 literally asks for the full request URL,
which one resource identifier for every REST path cannot satisfy.

A `401` for a missing or invalid bearer (`INVALID_CREDENTIALS`) on any REST route outside the
OAuth group carries `WWW-Authenticate: Bearer resource_metadata="<this metadata URL>"`; every
bearer-authenticated route there accepts Aura access tokens. REST CORS exposes the header. The
OAuth group gets no challenge: protocol endpoints keep the errors in
[OAuth](oauth.md#protocol-errors-and-cache-prevention), and consent, approval and OAuth client
administration are first-party flows that take a Cognito session, not an Aura access token. Other
`401` codes, such as webhook signature failures, involve no bearer and get no challenge.

The challenge names no `error` or `scope`; each operation's scopes are in [OpenAPI](swagger.yaml).

## Changing a document

Change the renderer and its tests together with this document. For the API host, also update
[OpenAPI](swagger.yaml) and the [changelog](CHANGELOG.md). A new path also needs an exact
Gateway route, its CloudFront behaviour and a row in the
[stage verification table](http-api-front-door.md#mcp-deployment-and-stage-verification).
