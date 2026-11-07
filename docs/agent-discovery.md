# Agent discovery

The backend writes every machine-readable document that describes Aura to agents. The API and
MCP hosts serve the documents about themselves. The API also serves the
[website documents](#website-documents), which the webapp publishes on the website origin but
never writes. Each document is built from the code and configuration of the service it
describes, so it cannot drift from what that service runs.

This document owns the discovery contract. [OAuth](oauth.md#authorization-server-discovery)
owns authorization-server metadata, [MCP](mcp.md#operation-families-and-admission) owns the MCP
protected-resource metadata and challenges, and the [front door](http-api-front-door.md) owns
edge routing, caching and stage verification.

## Environment

Every URL comes from the `oauth-core` resource policy that `OAUTH_ISSUER_URL` selects. Any other
issuer fails startup, so a document can never mix environments.

| Origin | Production | Stage |
| --- | --- | --- |
| Website (identified by its domain) | `https://aura-historia.com` | `https://stage.aura-historia.com` |
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
| [Website documents](#website-documents) | API `GET`/`HEAD /v1/site-discovery/…`, published on the website | Per document | Shared, 300 s |

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
OAuth group carries `WWW-Authenticate: Bearer resource_metadata="<this metadata URL>"`. Every
bearer-authenticated route there accepts Aura access tokens, although issuing a personal access
token and disconnecting an OAuth client then require a first-party Cognito session and refuse an
Aura access token with `403`. REST CORS exposes the header. The
OAuth group gets no challenge: protocol endpoints keep the errors in
[OAuth](oauth.md#protocol-errors-and-cache-prevention), and consent, approval and OAuth client
administration are first-party flows that take a Cognito session, not an Aura access token. Other
`401` codes, such as webhook signature failures, involve no bearer and get no challenge.

A presented bearer that is malformed, invalid, expired or revoked adds `error="invalid_token"`
([RFC 6750 §3.1](https://www.rfc-editor.org/rfc/rfc6750#section-3.1)), so a client refreshes or
signs in again; a missing bearer names no error. The challenge names no `scope`; each operation's
scopes are in [OpenAPI](swagger.yaml).

## Website documents

These documents describe the website `aura-historia.com`, not an API host: Poppy, for example,
requires `organization.domain` to equal the host that serves it. The pure
[`agent-discovery`](../src/agent-discovery) crate renders them from the environment and from
Markdown kept in that crate; the API renders them once per process and serves them below
`/v1/site-discovery/`. The webapp publishes each body and media type unchanged at its website
path, where agents and agent-readiness scanners look for it.

| Document | API path below `/v1/site-discovery/` | Website path | Media type |
| --- | --- | --- | --- |
| [AI catalog](#ai-catalog) | `ai-catalog.json` | `/.well-known/ai-catalog.json` and `/.well-known/ard.json` | `application/ai-catalog+json` |
| [Poppy declaration](#poppy-declaration) | `poppy.json` | `/.well-known/poppy.json` | `application/json` |
| [Agent skills index](#agent-skills) | `agent-skills/index.json` | `/.well-known/agent-skills/index.json` | `application/json` |
| [Agent skill](#agent-skills) | `agent-skills/{name}/SKILL.md` | `/.well-known/agent-skills/{name}/SKILL.md` | `text/markdown; charset=utf-8` |
| [auth.md](#authmd) | `auth.md` | `/auth.md` | `text/markdown; charset=utf-8` |

The routes are exact; a skill name the index does not list returns `404`. Responses are
approved anonymous discovery representations with a 300-second shared TTL, like the API
catalog, and allow any origin. Links between the documents use their website URLs.

### AI catalog

The catalog follows [Agentic Resource Discovery](https://agenticresourcediscovery.org/) in the
ai-catalog data model, whose `specVersion` is `1.0`. Its `host` is the website, identified by
its domain because Aura publishes no DID document. Its three entries link to the
[MCP Server Card](#mcp-server-card), the OpenAPI document and the
[agent skills index](#agent-skills). Each has a `urn:air:<website domain>:<namespace>:<name>`
identifier, a display name, the linked document's media type, a description and two to five
representative queries. ARD 0.91 makes `/.well-known/ard.json` the path consumers must fetch
and reads the same `entries`, while scanners still probe `ai-catalog.json`, so the webapp
publishes the same body at both paths.

### Poppy declaration

The [Personal Agent Protocol](https://personalagentprotocol.org/) declaration uses protocol
version `0.1`, the version agent-readiness scanners validate; move to a later version once they
validate it, because pre-1.0 agents read only their own `major.minor`. `organization.domain`
is the website's domain. It declares the website only, as `web: {}`, so agents browse it as
signed-out visitors. In `0.1`, declaring `apis` requires an `auth.issuer` whose
authorization-server metadata lists the domain in `poppy_domains` (later drafts bind through
`protected_resources`), and every listed API must accept Poppy session tokens. Aura does
neither, so the declaration has no `apis` and no `auth`.

### Agent skills

The index follows [Agent Skills Discovery](https://github.com/cloudflare/agent-skills-discovery-rfc)
v0.2.0. Each skill is one `SKILL.md`: YAML front matter whose `name` matches the skill's
directory and whose `description` says what the skill does and when to use it, then Markdown
instructions. Each index entry has type `skill-md`, the front matter's name and description,
the skill's website URL and the `sha256:` digest of the skill. A skill's Markdown is compiled
into the binary with placeholders for the environment's values; the digest covers the rendered
bytes, which are exactly the bytes served.

| Skill | Covers |
| --- | --- |
| `catalogue-search` | MCP tools and REST routes for the public catalogue, continuation rules, and that Aura is a discovery service, not a marketplace |
| `connect-account` | Signing a user in from an agent, scopes, refreshing tokens and disconnecting |
| `partner-listing-batches` | Synchronous and asynchronous partner listing batches over REST, with their retry rules |

### auth.md

`auth.md` tells agents how to authenticate. Its first heading contains `auth.md`. It covers
anonymous access, signing a user in from an agent with a Client ID Metadata Document and
refreshing its tokens, administrator-registered clients, personal access tokens and
disconnecting, with the environment's issuer, metadata URLs and user scopes. It describes [OAuth](oauth.md) and states no token lifetimes:
clients read them from the token response. It has no `agent_auth` block, and the
authorization-server metadata advertises none, because Aura registers no agent identities.
Scanners that require `agent_auth` or a registration flow therefore do not pass it.

## Changing a document

Change the renderer and its tests together with this document. For the API host, also update
[OpenAPI](swagger.yaml) and the [changelog](CHANGELOG.md). A new path also needs an exact
Gateway route, its CloudFront behaviour and a row in the
[stage verification table](http-api-front-door.md#mcp-deployment-and-stage-verification); a new
website document also needs its publication in the webapp. Editing a skill changes its digest;
the index follows automatically. Keep skills and `auth.md` in line with the contracts they
summarize, and link only to the environment's origins; tests reject a REST route or MCP tool
they name that the API or MCP server does not offer.
