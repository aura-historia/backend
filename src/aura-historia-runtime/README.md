# Shared runtime composition

`aura-historia-runtime` constructs concrete adapters and exposes typed inbound use-case
handles. REST and MCP call those handles directly. This crate does not own transport state,
DTOs, authentication, business rules, transaction boundaries or publication deadlines.

The initial extracted family is public listing-source discovery: search and detail by slug.
`Runtime::public_listing_sources()` constructs its handles once for the warm pool. It reads no
Stripe, Loops, webhook, Cognito, OpenSearch or Google configuration. Add another explicit family
only with the task that exposes its operations; keep provider configuration inside that selected
family's composition. Shared DTOs and pure mappings belong to `aura-historia-contracts` as each
family migrates.

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

`Runtime::from_pool()` supports caller-owned fixture pools. `postgres_pool()` remains a composition
interface for REST families awaiting extraction; request handlers must call service use cases.
The existing REST authenticators, optional-auth rejection, HTTP budgets and response mapping stay
in the REST adapter. Native and Lambda entry points inject the same family handles.

The runtime unit suite checks clean-environment anonymous startup, warm reuse, credential-version
replacement, refresh failure and use-case error causes. Its subprocess probes isolate environment
configuration without mutating a multithreaded test process. PostgreSQL integration tests use the
repository's isolated Docker fixture and business migrations.
