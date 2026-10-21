# Shared runtime composition

`aura-historia-runtime` constructs concrete adapters and exposes typed inbound use-case
handles. REST and MCP call those handles directly. This crate does not own transport state,
DTOs, authentication, business rules, transaction boundaries or publication deadlines.

`Runtime::public_listing_sources()`, `public_party_locations()` and `public_overview()`
construct public source, location and catalogue aggregate handles once for the warm pool,
without loading unrelated provider configuration. Listing discovery selects
`Runtime::public_product_listings(ProductListingDependencies)` with explicit OpenSearch and
embedding-provider handles and the existing cache/read policies. Its detail, similarity and search
handles are reused for the warm pool. `ProductListingConfig` parses the existing listing-only
Vertex, cache and read-policy settings; `ProductListingDependencies::from_env()` constructs
OpenSearch and the lazy embedding adapter for REST and MCP. ADC is resolved only when an
embedding is requested. Lambda entry points reuse the synchronous ADC setup in
`platform-lambda-bootstrap` before starting worker threads. `Runtime::listing_shipping()` shares
the existing shipping assessment/address-resolution composition with catalogue readers. Constructing these handles
performs no provider request. Shared DTOs and pure mappings belong to `aura-historia-contracts`.
Add further explicit families only with the task exposing their operations; keep configuration
requirements confined to the selected family.

`Runtime::public_product_listing_history()` independently composes the existing complete-history
use case from PostgreSQL and historical FX readers. It requires no search or embedding providers;
REST and MCP reuse the same handle.

`Runtime::public_auctions()` supplies public directory, detail and catalogue handles.
Its catalogue reuses the runtime's listing shipping enrichment from the same pool and the
existing transactional catalogue/FX use case; selecting auctions needs no search,
embedding or identity-provider configuration.

Native entry points use `Runtime::from_env()` with the existing explicit PostgreSQL settings.
Lambda entry points use `RotatingRuntime::from_env()` and call `current()` before each invocation.
It reuses `platform-lambda-bootstrap` configuration and versioned composition leases, plus the
existing PostgreSQL credential provider. One credential version shares its pool and selected
handles; a version change composes a replacement without closing pools held by earlier leases.
Credential refresh failure rejects the invocation without falling back to cached credentials.
Composition errors preserve their source chain and use safe messages.

```rust,ignore
let lease = runtime.current().await?;
let services = lease.value().public_listing_sources();
let result = services.search.execute(&context, request).await?;
```

`Runtime::watchlist()` composes the four existing watchlist use cases once for the warm pool.
REST and MCP reuse these typed handles, including transactional tier/quota enforcement,
listing presentation and shared shipping enrichment. Selecting this family requires PostgreSQL
without loading search, embedding, billing or identity-provider configuration.

`Runtime::access_token_authentication()` composes the existing opaque-token authentication
use case against the same environment pool. REST and MCP can reuse its typed handle without
loading Cognito or unrelated provider configuration. The handle is reusable; authentication
results are never cached. The PostgreSQL reader preserves database revocation and suspension
checks on each request. Transport adapters own parsing, principal mapping and challenges.

`Runtime::from_pool()` supports caller-owned fixture pools. `postgres_pool()` remains a composition
interface for REST families awaiting extraction; request handlers must call service use cases.
`PublicListingSourceReadBudget` supplies the shared non-queuing permit pool and per-read
timeout for source search and slug detail. Transports share one cloneable budget across those
operations, hold its permit for active work, and cap execution by the invocation deadline. Its
default is the existing REST policy (four concurrent reads, 500 ms); REST keeps its configured
bounds and authentication within the same read budget. The existing REST authenticators,
optional-auth rejection and response mapping stay in the REST adapter. Native and Lambda entry
points inject the same family handles.

The runtime unit suite checks clean-environment anonymous startup, warm reuse, credential-version
replacement, refresh failure and use-case error causes. Its subprocess probes isolate environment
configuration without mutating a multithreaded test process. PostgreSQL integration tests use the
repository's isolated Docker fixture and business migrations.
