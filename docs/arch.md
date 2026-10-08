# Architecture Guide

This document defines architecture, design boundaries, and guardrails for this workspace. It is intended to be read before adding or changing domain logic, use cases, persistence, APIs, integrations, or projections. Code blocks, example directory trees, and `Record` names are illustrative patterns, not prescribed implementations or an inventory of current crates. Keep the rules, rationale, and examples explicit enough to show how to apply the boundaries; brevity is not a goal at the expense of that guidance.

Specialized documents own public, storage, event, deployment, and operational contracts; code and configuration own concrete wiring, values, and mechanics. See the [documentation map](README.md). Deployment procedures belong in [infrastructure](infra.md); integration-specific consent, proof, provider, and evidence rules belong in [marketing consent](marketing-consent.md), not in this guide. Changes that intentionally deviate from this guide MUST explain the reason in the pull request and SHOULD update it when the deviation represents a new general rule.

---

## 1. Normative language

The terms **MUST**, **MUST NOT**, **SHOULD**, **SHOULD NOT**, and **MAY** are normative.

- **MUST / MUST NOT**: required for architectural consistency.
- **SHOULD / SHOULD NOT**: strong default; deviations require a concrete reason.
- **MAY**: optional.
- Examples illustrate the rules but do not override them.

---

## 2. Architecture at a glance

The workspace follows Domain-Driven Design, clean dependency boundaries, ports and adapters, and command/query separation.

```text
Transport
REST controllers, workers, CLI
        │
        │ calls inbound use-case contracts
        ▼
Application / service
use-case requests, commands, results, views, errors
use-case handler implementations
outbound capability and transaction ports
        │
        │ calls outbound ports
        ▼
Adapters
PostgreSQL, search engines, key-value stores,
graph/knowledge stores, external APIs, queues
        │
        ▼
External systems
```

The diagram shows runtime call flow, not Rust dependency direction. Adapters depend on service-owned contracts, and services depend on core types—not the other way around. See [dependency direction](#36-dependency-direction).

The write model is authoritative in PostgreSQL unless a bounded context explicitly documents another operational source of truth.

Additional data sources are adapters. They MAY provide:

- search;
- fast key-value reads;
- graph or semantic enrichment;
- user-specific state;
- analytics;
- recommendations;
- external metadata;
- rebuildable read projections.

They MUST NOT silently become authoritative for domain invariants.

### Core rules

1. Controllers call use cases.
2. Use-case contracts, input/output types, and handler implementations belong to the corresponding `<entity>-service` crate.
3. Domain behavior belongs to the corresponding `<entity>-core` crate.
4. Ports describe application capabilities, not databases.
5. Adapter crates implement service-owned ports.
6. Handlers depend only on core types and service-owned ports; they MUST NOT import SQLx, database clients, SDK clients, rows, documents, or adapter implementations.
7. Storage representations remain private to their adapter crate.
8. Repositories reconstruct and persist aggregates.
9. Readers build read models.
10. Read models are not aggregates.
11. Write handlers define transaction scope through an abstract `UnitOfWork` and transaction-bound repository factories.
12. Several PostgreSQL repositories MAY participate in the same abstract transaction.
13. Cross-datasource writes do not share a transaction.
14. CDC and projection invariants are summarized below; event routing and recovery belong to the dedicated event and worker documentation.
15. REST DTOs belong to the REST layer and are mapped by controllers.
16. Trusted caller identity is mapped into a service-owned `OperationContext`.
17. Domain and service crates MUST NOT depend on infrastructure crates.

## 3. Canonical workspace layout

Bounded contexts separate domain, service, and adapter responsibilities. The following `record` layout illustrates dependency direction, not required directory names or a current crate inventory. Only adapters actually needed should exist.

```text
Cargo.toml

record-core/
record-service/
record-postgres/
record-opensearch/

workspace-core/
workspace-service/
workspace-postgres/

platform-postgres/      # shared concrete SQLx transaction primitives when required
api/
runtime/                # composition root and process startup
```

### 3.1 Core crate

```text
record-core/
└── src/
    ├── lib.rs
    ├── record.rs
    ├── record_id.rs
    ├── workspace_id.rs
    ├── value_objects.rs
    ├── events.rs              # optional
    ├── policies.rs
    └── errors.rs
```

`record-core` owns domain state and behavior. It MUST NOT depend on `record-service` or any adapter crate.

### 3.2 Service crate

```text
record-service/
└── src/
    ├── lib.rs
    ├── operation_context.rs
    ├── transaction.rs
    ├── use_cases/
    │   ├── mod.rs
    │   ├── commands/
    │   │   ├── create_record.rs
    │   │   ├── rename_record.rs
    │   │   └── archive_record.rs
    │   └── queries/
    │       ├── search_records.rs
    │       └── get_record_details.rs
    └── ports/
        ├── record_repository.rs
        ├── record_search_reader.rs
        ├── record_details_reader.rs
        ├── record_user_state_reader.rs
        └── record_metadata_reader.rs
```

Each use-case file SHOULD contain:

- the command or request;
- the result or final view;
- the use-case error;
- the inbound use-case trait;
- the concrete handler implementation.

`record-service` depends on `record-core` and public contracts from other core/service crates when a use case genuinely spans entities.

### 3.3 PostgreSQL adapter crate

```text
record-postgres/
└── src/
    ├── lib.rs
    ├── repository_factory.rs
    ├── repositories/
    │   └── record_repository.rs
    ├── readers/
    │   ├── record_details_reader.rs
    │   └── record_user_state_reader.rs
    ├── rows/
    │   ├── record_row.rs
    │   └── record_details_row.rs
    └── mapping.rs
```

`record-postgres` owns SQLx rows, SQL, mappings, transaction-bound repository implementations, reader implementations, and the concrete factories required by the composition root.

It MUST NOT own use-case handlers.

### 3.4 Other adapter crates

```text
record-opensearch/
└── src/
    ├── lib.rs
    ├── record_document.rs
    ├── record_search_reader.rs
    └── projector.rs
```

Technology-specific names are appropriate for adapter crates because they describe the implementation boundary.

### 3.5 Transport and composition root

Transport crates authenticate and map requests or jobs to inbound use cases; the composition root constructs concrete adapters and injects them into service-owned handlers. Composition roots MAY depend on every crate needed to assemble the process but MUST NOT contain business behavior or implement service-owned ports. Route and worker handlers MUST NOT construct repositories or clients or own transaction boundaries. Protected endpoint authorization belongs in use cases or service-owned policies, not controllers.

Scheduled executables SHOULD be run-to-completion callers of service use cases; durable coordination belongs in authoritative storage rather than process-local state. Operational migration tooling is separate from application runtimes and MAY use direct operational SQL, but domain/service crates MUST NOT depend on it. Application startup MUST NOT silently run business migrations. Deployment order, exception details, and migration admission are owned by the [infrastructure guide](infra.md).

### 3.6 Dependency direction

```text
record-core
    ▲
    │
record-service
    ▲
    │
record-postgres / record-opensearch
    ▲
    │
api and runtime
```

Allowed dependencies:

```text
record-service       -> record-core
record-postgres      -> record-service + record-core + platform-postgres as needed
record-opensearch      -> record-service + record-core identifiers as needed
api                  -> record-service + public core identifiers/value objects as needed
runtime              -> service crates + adapter crates + platform crates
```

Forbidden dependencies:

```text
record-core          -X-> record-service
record-core          -X-> adapters
record-service       -X-> adapters
record-service       -X-> REST DTOs
adapter A            -X-> private types from adapter B
controller           -X-> concrete database client
controller           -X-> repository
controller           -X-> storage row/document/item
```

Cross-entity use cases MUST have one clear owning service crate. If no existing entity service is a natural owner, create a dedicated application/service crate rather than introducing cyclic dependencies.

### 3.7 Durable shared crates

Shared crates MUST own narrow, proven cross-context concepts, not become a replacement `common` hub. For example, `application` owns technology-neutral application contracts; `domain-primitives` owns proven domain-neutral values; `money` and `localization` own pure semantic values; platform crates own reusable infrastructure mechanics, not bounded-context behavior. Core, service, adapter, and transport crates SHOULD import the actual owner directly. Provider crates own their protocol and vocabulary; bounded-context adapters own their storage rows, search documents, queries, and mapping. Legacy `common` MUST NOT gain new canonical consumers. Do not move a type to a shared crate solely because two call sites look similar.

## 4. Domain-Driven Design boundaries

### 4.1 Aggregates

An aggregate is the consistency boundary for synchronous domain rules.

An aggregate MUST:

- own its invariants;
- expose behavior through methods;
- keep fields private;
- prevent invalid state transitions;
- refer to other aggregates by typed identifier.

An aggregate MAY emit domain events for meaningful completed state changes. Domain events are an optional modeling mechanism, not a requirement for every aggregate.

```rust
pub struct Record {
    id: RecordId,
    workspace_id: WorkspaceId,
    title: RecordTitle,
    status: RecordStatus,
}

impl Record {
    pub fn rename(
        &mut self,
        new_title: RecordTitle,
    ) -> Result<(), RenameRecordError> {
        if self.status.is_archived() {
            return Err(RenameRecordError::Archived);
        }

        if self.title == new_title {
            return Ok(());
        }

        self.title = new_title;

        Ok(())
    }
}
```

An event-driven aggregate MAY additionally collect domain events internally:

```rust
self.pending_events.push(RecordEvent::Renamed {
    record_id: self.id,
});
```

Non-event-driven aggregates MUST NOT introduce event machinery merely for architectural uniformity.

An aggregate MUST NOT contain a hydrated object from another aggregate:

```rust
// Forbidden
pub struct Record {
    workspace: Workspace,
}
```

It stores the reference instead:

```rust
pub struct Record {
    workspace_id: WorkspaceId,
}
```

Hydrated cross-aggregate information belongs to read models.

#### Operational metadata

Operational metadata such as `created_at`, `updated_at` is not aggregate state unless a domain invariant explicitly depends on it.

It MUST NOT be added to an aggregate merely for audit, display, sorting, or transport compatibility.

This metadata lives in the persistence layer. The repository owns writing and updating it from service-provided write metadata, clocks, and persistence defaults. When reconstructing an aggregate, the repository MUST map only domain state back into the aggregate.

Access to operational metadata belongs to dedicated readers and read use cases. A details, audit, or history reader MAY return metadata in an application-owned read model. The aggregate repository MUST NOT expose metadata just to satisfy presentation needs.

### 4.2 Domain types

`core` owns:

- aggregates;
- entities internal to aggregates;
- value objects;
- typed identifiers;
- domain policies that are pure;
- domain events, when the aggregate uses them;
- domain errors.

`core` MUST NOT depend on:

- SQLx;
- Serde solely for transport or persistence;
- HTTP frameworks;
- search clients;
- cloud SDKs;
- queue clients;
- `tracing`;
- environment variables;
- database rows or documents.

Domain code SHOULD be deterministic and testable without mocks, databases, clocks, networks, or runtimes. Time, identifiers, randomness, and external decisions MUST be supplied as values or through explicit application ports when needed.

### 4.3 Semantic absence and orthogonal dimensions

A valid absence of a semantic assertion MUST use `Option<T>`, not an `Unknown` enum variant. Uncertainty, unsupported external values, and extraction failure belong to the boundary adapter and MUST NOT be persisted as invented core truth. Orthogonal domain dimensions, such as catalog lifecycle and availability, MUST use separate types. A broad classification derived from a concrete domain value MAY be exposed for reads and queries, but MUST NOT be independently mutable or persisted.

## 5. Type ownership and visibility

### 5.1 Ownership table

| Type category | Example | Owner | Default visibility |
|---|---|---|---|
| Aggregate | `Record` | `record-core` | `pub`, fields private |
| Value object | `RecordTitle` | `record-core` | `pub`, fields private |
| Typed ID | `RecordId` | `record-core` or shared identifiers crate | `pub` |
| Domain event, when used | `RecordEvent` | `record-core` | private, `pub(crate)`, or `pub` only when consumed across crates |
| Principal/context | `Principal`, `OperationContext` | `record-service` or shared application crate | `pub` |
| Use-case command | `RenameRecordCommand` | `record-service::use_cases` | `pub` |
| Query request | `SearchRecordsRequest` | `record-service::use_cases` | `pub` |
| Use-case result | `RenameRecordResult` | `record-service::use_cases` | `pub` |
| Read model/view | `RecordSummary` | `record-service::use_cases` or `ports` | `pub` when an adapter/controller consumes it |
| Inbound use-case trait | `RenameRecordUseCase` | `record-service::use_cases` | `pub` |
| Use-case handler | `RenameRecordHandler` | `record-service::use_cases` | `pub` when wired externally; fields private |
| Outbound port | `RecordRepository`, `RecordDetailsReader` | `record-service::ports` | `pub` because adapter crates implement it |
| Transaction abstraction | `UnitOfWork`, `Transaction` | service/shared application crate | `pub` |
| PostgreSQL factory | `SqlxRecordRepositoryFactory` | `record-postgres` | `pub` when required by runtime wiring |
| PostgreSQL scoped repository | `SqlxRecordRepository` | `record-postgres` | private whenever the factory return type can remain opaque |
| PostgreSQL row | `RecordRow` | `record-postgres` | private or `pub(crate)` |
| Search document | `RecordDocument` | `record-opensearch` | private or `pub(crate)` |
| Shared search protocol envelope | `SearchResponse<T>` | `platform-opensearch` | `pub` only after multiple production adapters prove the exact generic wire shape |
| REST request DTO | `RenameRecordRequestDto` | `api` | private or `pub(crate)` |
| REST response DTO | `RecordDetailsResponseDto` | `api` | private or `pub(crate)` |

### 5.1.1 Semantic leaf types across boundaries

Architectural ownership of a DTO, command, read model, row, or document does not mean every field needs a layer-local type. Boundary structures SHOULD reuse public canonical identifiers, value objects, and enums when meaning and the intended value set are identical, reuse preserves dependency direction, and no technology representation escapes its owner.

A boundary-local field type SHOULD exist only for a distinct vocabulary, compatibility rule, validation rule, unknown-value policy, representation metadata, or independent lifecycle. A variant-for-variant mirror enum with only identity conversions is a smell and needs a concrete reason.

REST owns request/response structure, JSON field names, omission and null rules, aliases, REST-specific compatibility vocabulary, and HTTP validation. A semantic owner MAY also own a stable canonical machine identifier when that identifier is genuinely shared across first-party representations. REST may expose that identifier through an API-local codec without adding Serde to core types; if REST vocabulary diverges, the API-local codec owns the divergent mapping. REST does not own a duplicate of every semantic field.

PostgreSQL owns row types and storage encoding. A row MAY decode directly into canonical semantic leaf types; an intermediate storage enum is unnecessary unless it carries distinct storage semantics.

```rust
#[derive(serde::Deserialize)]
struct CreateListingSourceDto {
    name: ListingSourceName,
    #[serde(with = "ingestion_method_wire")]
    ingestion_methods: Vec<ListingIngestionMethod>,
}
```

Avoid a boundary mirror such as `ListingIngestionMethodDto` plus exhaustive identity conversions unless an intentional contract divergence requires it.

For bounded-context enums, this workspace uses the policy that the canonical value set equals the REST value set when the API delegates to the canonical identifier. Adding a canonical variant therefore intentionally makes it REST-visible. A REST-specific alias, subset, or compatibility value still requires an API-local codec and review.

### 5.1.2 Object identifiers

Aura object identities use concrete semantic newtypes backed by UUIDv7. Domain, REST, semantic worker, logs, and rebuildable search documents use strict prefixed TypeID text. PostgreSQL PK/FK columns remain native `uuid`; CDC reflects that storage form and maps it immediately into typed IDs. Internal persisted JSON uses an adapter-owned explicit UUID codec where documented and MUST NOT derive storage encoding from an ID's `Display` implementation. Bare UUID object-ID input and wrong prefixes are invalid. The complete registry, exclusions, and addition recipe live in [`object-ids.md`](object-ids.md).

### 5.2 Visibility rules

Use the narrowest visibility that satisfies a real production crate boundary.

- Items are private by default.
- Aggregate fields MUST be private.
- Use `pub(super)` only for a parent module inside the same crate.
- Use `pub(crate)` only for cross-module access inside the same crate.
- Use `pub` only when another production crate must use or implement the item.
- Service-owned ports MUST be `pub` because adapter crates implement them.
- Use-case handlers and constructors MUST be `pub` only when the composition root constructs them directly.
- Concrete adapter factories/readers MUST be `pub` only when the composition root or a black-box consumer needs them.
- Adapter rows, mapping helpers, SQL parameter structs, concrete transaction-scoped repositories, and adapter-specific client response types MUST remain private or `pub(crate)`. A narrow platform crate MAY expose a generic protocol envelope only when multiple production adapters require the exact shape and no bounded-context storage representation escapes.
- Fields of public adapter types MUST remain private.
- Do not expose a public constructor for a type that consumers should obtain only through a factory.
- Do not widen visibility solely for tests.

A public item is part of the workspace architecture contract even when the workspace is not published to crates.io.

### 5.3 Opaque transaction-scoped implementations

Repository factories SHOULD use return-position `impl Trait` so the concrete transaction-scoped repository can remain private:

```rust
pub trait RecordRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl RecordRepository + 'tx;
}
```

The adapter may then keep the implementation private:

```rust
pub struct SqlxRecordRepositoryFactory;

struct SqlxRecordRepository<'tx> {
    tx: &'tx mut SqlxTransaction,
}
```

If language or trait constraints require a public concrete type, expose only the minimum surface, keep its fields private, and avoid public construction outside the factory.

### 5.4 Rehydration boundary

Because the PostgreSQL adapter is a separate crate, aggregate rehydration APIs used by adapters must be deliberately `pub`.

```rust
impl Record {
    #[doc(hidden)]
    pub fn rehydrate(state: RehydratedRecordState) -> Result<Self, RehydrateRecordError> {
        // Validate persisted state without emitting new events.
        todo!()
    }
}
```

This is an adapter-facing construction boundary, not a general mutation API. Its input fields SHOULD use domain types where practical, and all aggregate fields remain private.

### 5.5 Test visibility

Tests inside a source file under `#[cfg(test)] mod tests` can access that file's private items and MAY run against real infrastructure.

Tests in a crate-level `tests/` directory compile as separate crates and MUST use only the deliberate public API.

Therefore:

- private implementation and real-infrastructure adapter tests belong beside the implementation;
- black-box contract tests belong in `/tests`;
- implementation details MUST NOT be made `pub` merely so a `/tests` test can access them.

## 6. Use cases

Reads and writes are both use cases.

Each use case SHOULD be focused and owned by the corresponding service crate. The following `Record` code is illustrative, not a required file structure or API:

- command or request;
- result or final view;
- use-case error;
- inbound use-case trait;
- the concrete handler implementation.

### 6.1 Write use-case contract

```rust
// record-service/src/use_cases/commands/rename_record.rs

pub struct RenameRecordCommand {
    pub record_id: RecordId,
    pub new_title: String,
}

pub struct RenameRecordResult {
    pub record_id: RecordId,
    pub title: String,
}

#[derive(Debug, thiserror::Error)]
pub enum RenameRecordError {
    #[error("record not found")]
    NotFound,

    #[error("concurrent record update")]
    ConcurrencyConflict,

    #[error("invalid title")]
    InvalidTitle,

    #[error("authenticated actor required")]
    AuthenticatedActorRequired,

    #[error("temporary persistence failure")]
    TemporarilyUnavailable,

    #[error("internal failure")]
    Internal,
}

#[async_trait::async_trait]
pub trait RenameRecordUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: RenameRecordCommand,
    ) -> Result<RenameRecordResult, RenameRecordError>;
}
```

Commands SHOULD express business intent:

```text
CreateRecord
RenameRecord
ArchiveRecord
RestoreRecord
PublishRecord
```

Avoid generic update commands with weak intent:

```text
UpdateRecord {
    title: Option<String>,
    status: Option<String>,
    owner: Option<Uuid>,
    ...
}
```

When a public PATCH endpoint is intentionally broad, the service owns a corresponding update command with explicit tri-state fields (`Unchanged`, `Set(value)`, `Clear`) so omitted, null, and set values remain distinct. Shared technology-neutral patch semantics belong in an application owner, not core.

The update handler MUST translate command fields into explicit aggregate methods such as `change_title`, `replace_address`, or `replace_contact`, and track `ChangeOutcome`. Generic update MUST NOT include state-machine transitions that deserve their own use case, such as publishing, archiving, or changing aggregate status.

A broad update use case is one logical write. Domain methods SHOULD distinguish changes from no-ops without knowing storage versions. The handler MUST skip persistence when nothing changed and persist the complete authoritative state once when it did, enforcing optimistic concurrency. The storage version is internal persistence/CDC state and MUST NOT be returned from ordinary use cases.

### 6.2 Read use-case contract

```rust
// record-service/src/use_cases/queries/search_records.rs

pub struct SearchRecordsRequest {
    pub text: String,
    pub page: PageRequest,
}

pub struct RecordSummary {
    pub record_id: RecordId,
    pub title: String,
    pub container_name: String,
    pub is_watched: bool,
    pub is_liked: bool,
}

pub struct SearchRecordsResult {
    pub items: Vec<RecordSummary>,
    pub total: u64,
    pub next_page: Option<PageToken>,
}

#[async_trait::async_trait]
pub trait SearchRecordsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: SearchRecordsRequest,
    ) -> Result<SearchRecordsResult, SearchRecordsError>;
}
```

The final read model is owned by the use case, not by any data source.

### 6.3 One implementation per use case

A use case SHOULD have one focused handler implementation in the service crate.

Preferred:

```text
RenameRecordUseCase
    implemented by RenameRecordHandler

SearchRecordsUseCase
    implemented by SearchRecordsHandler

GetRecordDetailsUseCase
    implemented by GetRecordDetailsHandler
```

Avoid one large implementation:

```rust
// Forbidden direction
pub struct RecordService<
    Repository,
    Search,
    Details,
    Metadata,
    UserState,
    ...
> {
    // dependencies for every unrelated use case
}
```

Each handler MUST depend only on the capabilities it uses.

### 6.4 Handler location and dependencies

All use-case handler implementations live in the corresponding `<entity>-service` crate.

```text
record-service/src/use_cases/commands/rename_record.rs
    RenameRecordCommand
    RenameRecordResult
    RenameRecordError
    RenameRecordUseCase
    RenameRecordHandler
```

Handlers MUST work exclusively against:

- core types;
- service-owned repository/reader/writer ports;
- service-owned transaction abstractions;
- public contracts from another service/core crate when the use case genuinely spans entities.

Handlers MUST NOT import:

```text
sqlx
PgPool
PgConnection
OpenSearch clients
adapter rows/documents
concrete adapter factories
```

This gives one unambiguous rule: service crates implement application behavior; adapter crates implement infrastructure ports.

## 7. Inbound use-case traits and outbound ports

There are two directions of traits.

```text
REST/controller
    │
    │ calls inbound port
    ▼
RenameRecordUseCase
    │
    │ implemented in record-service
    ▼
RenameRecordHandler
    │
    │ calls outbound ports
    ▼
UnitOfWork + RecordRepositoryFactory + authorization readers
    │
    │ implemented by adapter crates
    ▼
PostgreSQL and other infrastructure
```

### Inbound use-case traits

Inbound use-case traits describe what callers may do.

Examples:

```text
CreateRecordUseCase
RenameRecordUseCase
SearchRecordsUseCase
GetRecordDetailsUseCase
```

They belong to the corresponding service crate.

Controllers MUST depend on inbound use-case traits, never concrete handlers or adapters.

### Outbound ports

Outbound ports describe capabilities needed by handlers.

Examples:

```text
RecordRepository
RecordRepositoryFactory
RecordSearchReader
RecordDetailsReader
RecordUserStateReader
RecordMetadataReader
UnitOfWork
Clock
AuthorizationPolicy
IdempotencyStore
```

They belong to service crates or a small shared application crate for genuinely cross-cutting abstractions.

Webhook verification MUST happen in a provider adapter behind a service-owned port and authenticate the original request before parsing. The transport owns acknowledgement; the service owns event meaning, durable receipts, idempotency, and application-state decisions.

- Processing MUST deduplicate by provider delivery identity, reject reuse with different content, and distinguish provider event time from receipt time.
- Unverifiable or malformed input MUST fail closed. Authenticated events outside supported application semantics MAY be ignored; provider-supplied identity MUST NOT be trusted as local user identity.
- Subscription, opt-out, bounce, and complaint remain distinct facts. Events and account/profile lifecycle changes MUST NOT by themselves grant consent.
- Errors and logs MUST NOT expose signing material, signatures, or sensitive webhook content.

A reusable capability crate MAY own a technology-neutral contract and its provider implementation when it contains no bounded-context behavior (for example embedding, classification, structured generation, or safe image retrieval). The consuming service owns application semantics, result mapping, concurrency, business retry policy, and input limits. The provider adapter owns authentication, wire encoding/decoding, provider-specific prompts or request format, configured model selection, deadlines, and provider error classification. Callers MUST NOT construct provider-specific instruction strings or choose provider/model identifiers through business requests unless selection itself is a documented product requirement.

### Third-party API guardrails

- Access external APIs through narrow service-owned capability ports and adapter-owned clients. Keep provider wire DTOs, SDK types, credentials, and provider vocabulary out of domain, service results, and transport contracts; explicitly validate and map external responses, including unknown/unsupported values.
- Construct credentials and clients at the composition boundary. Apply least-privilege scopes, secret redaction, approved egress destinations, TLS verification, and SSRF protections for externally supplied URLs. Never log raw provider payloads or secrets.
- Bound requests with timeouts, concurrency limits, and provider rate-limit/backoff policies. Retry only failures whose semantics allow it, with bounded jitter/backoff and idempotency where supported; distinguish definite rejection, confirmed acceptance, and ambiguous outcomes. A timeout MUST NOT be treated as proof of non-delivery.
- The service decides whether an external read is required, optional, or stale-tolerant and how failures affect the use case. For external writes, persist durable intent/receipts when needed and reconcile uncertain outcomes; never hold an authoritative database transaction across a provider call or assume an atomic cross-system commit.
- Minimize data shared with providers, validate provider responses before they influence business state, and document any provider-specific consent, retention, quota, cost, or outage requirements in the owning integration contract. For email-marketing integration rules, see [marketing consent](marketing-consent.md).

Ports MUST be named by capability, not by technology.

Preferred:

```text
RecordSearchReader
RecordMetadataReader
RecordUserStateReader
```

Forbidden:

```text
OpenSearchPort
PostgresReader
ExternalDatabasePort
```

One adapter may implement multiple ports. One port may have multiple implementations.

```text
RecordDetailsReader
    <- PostgresRecordDetailsReader

RecordSearchReader
    <- OpenSearchRecordSearchReader
    <- PostgresRecordSearchReader
```

There is not one port per data source.

Readers and repositories SHOULD receive only the narrow application data they require. They MUST NOT receive `OperationContext` or transport DTOs.

## 8. Repositories

### 8.1 Responsibility

A repository reconstructs and persists an aggregate.

```rust
pub type VersionedRecord = Versioned<Record, RecordStorageVersion>;

#[async_trait::async_trait]
pub trait RecordRepository: Send {
    async fn find_by_id(
        &mut self,
        id: RecordId,
    ) -> Result<Option<VersionedRecord>, RecordRepositoryError>;

    async fn insert(
        &mut self,
        record: &Record,
    ) -> Result<VersionedRecord, RecordRepositoryError>;

    async fn update(
        &mut self,
        record: &Record,
        expected_version: RecordStorageVersion,
    ) -> Result<VersionedRecord, RecordRepositoryError>;
}
```

The repository port is public because a separate adapter crate implements it.

Repository `insert` and `update` methods MUST return the persisted aggregate state, not `()`. When storage-generated metadata such as `created`, `updated`, or version is needed by the use case result, return a storage-neutral persisted model that contains the aggregate plus that metadata. SQL row types MUST still stay private to the adapter.

A repository MAY contain additional aggregate-relevant lookup methods when they are needed to reconstruct or enforce the aggregate boundary.

A repository MUST NOT become a general read API.

Forbidden:

```rust
trait RecordRepository {
    async fn search(...);
    async fn get_details_with_container(...);
    async fn get_user_likes(...);
    async fn recommendations(...);
    async fn analytics_dashboard(...);
}
```

Those capabilities belong to readers.

A transaction-bound repository is obtained through a service-owned factory:

```rust
pub trait RecordRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl RecordRepository + 'tx;
}
```

The factory allows a handler to use clean repository methods while binding several repositories to the same transaction.

### 8.2 Method naming

This project prefers explicit semantics:

```text
find_by_id
insert
update
```

`find_by_id` returns `Option`.

```rust
async fn find_by_id(
    &mut self,
    id: RecordId,
) -> Result<Option<Record>, RecordRepositoryError>;
```

A method named `get_by_id` MAY be used when absence is represented as an error:

```rust
async fn get_by_id(
    &mut self,
    id: RecordId,
) -> Result<Record, GetRecordError>;
```

`insert` means the aggregate MUST be new and MUST return the inserted aggregate state.

`update` means the aggregate MUST exist, MUST enforce optimistic concurrency through an internal loaded storage version, and MUST return the updated aggregate state.

A generic `save` SHOULD NOT be used unless new/existing semantics and storage-version handling are explicit.

A generic `delete` SHOULD NOT be used for normal domain behavior. Prefer a domain state transition:

```text
archive
withdraw
disable
retire
```

Physical deletion MUST be an explicit administrative or retention operation such as:

```text
purge_expired_draft
remove_personal_data
delete_unrecoverable_record
```

### 8.3 Operational truth

The authoritative repository is backed by the operational source of truth, normally PostgreSQL.

Search indexes, key-value projections, caches, graph stores, and external metadata sources MUST NOT implement the aggregate repository unless they are explicitly the authoritative write model for that bounded context.

---

## 9. Readers and read models

Readers provide purpose-specific read capabilities.

```rust
#[async_trait::async_trait]
pub trait RecordDetailsReader: Send + Sync {
    async fn find_details(
        &self,
        record_id: RecordId,
    ) -> Result<Option<RecordBaseDetails>, RecordDetailsReadError>;
}
```

A reader returns application-owned read models:

```rust
pub struct RecordBaseDetails {
    pub record_id: RecordId,
    pub title: String,
    pub container: ContainerSummary,
}

pub struct ContainerSummary {
    pub container_id: ContainerId,
    pub name: String,
}
```

It MUST NOT return:

- domain aggregates for display;
- PostgreSQL rows;
- search documents;
- key-value items;
- external-client response types.

### 9.1 Relational joins

A normalized PostgreSQL join used for presentation belongs in a reader, not an aggregate repository.

```sql
SELECT
    r.id AS record_id,
    r.title AS record_title,
    c.id AS container_id,
    c.name AS container_name
FROM records r
JOIN containers c ON c.id = r.container_id
WHERE r.id = $1
```

The adapter may know both tables without importing another adapter's private Rust types.

SQL/schema coupling does not require a Rust dependency on another domain aggregate or its row type.

### 9.2 Hydration

Hydration is application orchestration.

Example:

```text
SearchRecordsHandler
    │
    ├── RecordSearchReader
    │       -> search source
    │       <- ordered RecordSearchHit values
    │
    └── RecordUserStateReader
            -> batch query by actor and record IDs
            <- HashMap<RecordId, RecordUserState>

SearchRecordsHandler
    -> merge while preserving search order
    -> SearchRecordsResult
```

Ports:

```rust
#[async_trait::async_trait]
pub trait RecordSearchReader: Send + Sync {
    async fn search(
        &self,
        request: &SearchRecordsRequest,
    ) -> Result<SearchResult<RecordSearchHit>, RecordSearchReadError>;
}

#[async_trait::async_trait]
pub trait RecordUserStateReader: Send + Sync {
    async fn find_for_records(
        &self,
        actor_id: ActorId,
        record_ids: &[RecordId],
    ) -> Result<HashMap<RecordId, RecordUserState>, RecordUserStateReadError>;
}
```

Rules:

- User-specific hydration MUST be batched.
- A reader MUST NOT be called once per search hit.
- Search ordering MUST be preserved after hydration.
- Missing user-state rows SHOULD map to explicit defaults.
- If user state affects filtering or ranking, it MUST be part of the query strategy rather than filtering only the returned page afterward.

### 9.3 Multiple data sources

A use case may compose any number of readers:

```text
GetRecordDetailsHandler
    ├── RecordDetailsReader        -> PostgreSQL
    ├── RecordMetadataReader       -> additional data source
    └── RecordUserStateReader      -> PostgreSQL or key-value source
```

The handler owns the final result. Keep reusable item data and orthogonal per-user state separate with the shared wrapper:

```rust
pub struct RecordDetailsView {
    pub record_id: RecordId,
    pub title: String,
    pub container: ContainerSummary,
    pub metadata: MetadataSection,
}

pub type PersonalizedRecordDetailsView =
    application::personalized::Personalized<RecordDetailsView, RecordUserState>;
```

The API maps that wrapper to a required `item` plus optional `userState`; do not inline `user_state` into the reusable item view.

Partial models belong to the ports that require them:

```text
RecordBaseDetails
RecordMetadataView
RecordUserState
```

Adapters map their private representations into these application types.

The controller MUST NOT compose data sources.

### 9.4 Required and optional enrichment

The use case MUST explicitly decide whether additional data is required.

```rust
pub enum MetadataSection {
    Available(RecordMetadataView),
    Empty,
    TemporarilyUnavailable,
}
```

Do not represent source failure as genuine absence unless the product semantics explicitly accept that loss of distinction.

---

## 10. Mapping and serialization

Mapping belongs to the boundary that owns the source representation: REST DTOs in the API, PostgreSQL rows in PostgreSQL adapters, search documents in search adapters, and provider payloads in provider adapters. Storage and transport mapping MUST NOT be placed in core. A core crate MAY own semantic fields of a composite key, but boundary-specific string encodings and compatibility rules belong to the owning boundary.

Stable machine-readable identities for fieldless domain enums MUST use an explicit exhaustive canonical mapping; boundary decoders reject unknown or noncanonical persisted values rather than silently defaulting. Persisted enum values use `SCREAMING_SNAKE_CASE` where applicable; retain standardized identifiers such as ISO language codes. Database constraints, migrations, and API codecs must evolve with the semantic values. Do not derive persisted identifiers from Rust variant names.

### 10.1 Transport mapping

REST owns request/response structure, field names, null/omission rules, HTTP validation, and service-error mapping. Controllers map requests to service commands and service results to response DTOs; services MUST NOT depend on REST DTOs or status codes. Reuse canonical semantic leaf types when their meaning and value set coincide (Section 5); otherwise use a deliberate transport codec. Fallible parsing SHOULD return a typed error rather than accepting invalid input.

### 10.2 Persistence mapping

Adapter-owned row types MAY use `sqlx::FromRow`, but MUST remain private to the PostgreSQL adapter. Map rows into domain aggregates or application read models explicitly and fallibly. Aggregate rehydration MUST validate persisted invariants without emitting new domain events; malformed or incompatible persisted state is an operation error, not not-found or a default value. Joined presentation reads map to application read models, never to a hydrated cross-aggregate domain object.

Aggregate-to-storage serialization belongs inside the repository/DAO write operation so insert/update fields, generated metadata, and optimistic concurrency remain explicit. Avoid public cross-layer aggregate-to-row conversions. Repeated binding details MAY use private adapter-local helpers. PostgreSQL rows and search documents MUST NOT escape adapters; external API responses similarly map to application-owned types, with unknown values handled explicitly.

## 11. Transactions

### 11.1 Ownership

The service use-case handler defines transaction scope without importing SQLx. It begins an abstract unit of work, binds the required repositories and invariant-critical readers to the same transaction, performs domain behavior and authoritative writes, and explicitly commits on success. The adapter implements the transaction using PostgreSQL. Transport, queue consumers, and composition roots MUST NOT begin or commit business transactions on behalf of a service use case. Public inbound use cases SHOULD remain transaction-free for callers: transactional mechanics are private to their implementation.

A transaction abstraction exposes lifecycle, not entity-specific methods. Service-owned factories bind narrow repository or reader ports to it; concrete scoped implementations remain private to their adapters. Multiple entity-specific PostgreSQL ports MAY participate in one compatible transaction, but a pool-backed read on another connection MUST NOT substitute for a transaction-bound read that determines an invariant-critical write. Ordinary presentation reads MAY use standalone readers; several reads requiring one consistent snapshot need an explicit read transaction.

**Illustrative flow (not a prescribed handler implementation):**

```text
service handler
    -> begin UnitOfWork
    -> load aggregate and authoritative policy state in the same transaction
    -> authorize and apply domain behavior
    -> persist only if changed, checking the loaded storage version
    -> commit explicitly
    -> return the committed result
```

Dropping an uncommitted transaction is rollback, not commit. A write use case SHOULD build its result from committed state or its storage-neutral persisted result, not perform a presentation read after the write just to populate the response. Keep transactions short; perform slow external reads before opening one when safe, and revalidate authoritative state inside before writing.

### 11.2 Cross-system boundaries

A PostgreSQL transaction cannot atomically include a search engine, queue, key-value store, or third-party API. Services compose abstract ports across these boundaries without claiming distributed atomicity. For durable external effects, use an explicit intent/event and idempotent delivery/receipts as appropriate; failures and ambiguous results require a recovery policy. CDC and projection propagation follow Section 12 and the owning event documentation.

## 12. CDC and projection architecture

CDC propagates committed authoritative changes to workers and rebuildable read projections. The intended shape is PostgreSQL commit -> CDC transport/router -> durable scoped jobs -> workers -> service use cases and projection adapters. This is an architectural target, not proof that a given stage has activated or verified delivery. [Event flow](events/flow.md) owns routes, event/job schemas and consumer behavior; the [worker runbook](durable-worker-runbook.md) owns activation, custody, replay and recovery; [infrastructure](infra.md) owns deployment declarations and gates. Subsection numbers for schema evolution and required tests retain their established link targets.

### 12.1 Storage ownership and write contract

Every dataset MUST have one documented operational owner. PostgreSQL is authoritative business state unless a bounded context explicitly documents otherwise. Other stores MUST be classified as authoritative operational storage, rebuildable projection, external source, or cache; a hot read path does not make a projection authoritative. OpenSearch is rebuildable read state, not a write model. A transactional event journal MAY represent committed changes without being an event-sourced aggregate or a general outbox; its exact storage contract belongs in [storage](storage.md) and [event flow](events/flow.md).

Authoritative state and any required durable intent or event MUST commit in one local transaction. External projections and providers MUST NOT be updated inside that transaction. Domain invariants MUST NOT depend on projections being current or external writes being atomic with PostgreSQL.

### 12.2 Routing, acknowledgment, and durable custody

A router MAY fan out one committed change to multiple jobs. It MUST validate the entire route (source, version, operation, domain identity, destinations, and bounded payloads) before publishing any job for that change. Acknowledgment or checkpoint MUST mean that every required publication is confirmed; partial or uncertain publication remains retryable and can duplicate earlier sends. Invalid or unsupported input MUST fail closed rather than silently disappear. Transport identifiers are correlation, not business identity.

Delivery is **at least once within configured retention**, not exactly once or globally ordered. Workers acknowledge only confirmed complete outcomes; invalid jobs, nonterminal claims, failed effects, and uncertain commits remain retryable. A lost response or deletion can redeliver an already completed job. Failed router changes and worker jobs need distinguishable durable failure custody, and operator-controlled replay MUST preserve source identity and order where required. Queue names, batch sizes, retention windows, retry budgets, archive destinations, and live cutover evidence belong in the [event flow](events/flow.md), [runbook](durable-worker-runbook.md), and infrastructure configuration.

### 12.3 Idempotency, ordering, and projections

Handlers MUST tolerate duplicates, concurrent delivery, replay, and older changes arriving after newer ones. Use stable domain identities and source revisions rather than queue message IDs for idempotency. Target writes SHOULD enforce idempotency through conditional writes, unique constraints, or version checks; an older or equal source version MUST NOT overwrite newer projection state. An external side effect may have occurred even when finalization is lost; use fenced leases/receipts and explicit recovery rules, not an assumption of exactly-once execution.

For current-state invalidation, reread authoritative state and compare its revision with the triggering revision; do not record a stale trigger as processed current state. Historical consumers instead use their exact immutable fact. If correctness depends on current state at the final write, recheck under the authoritative transaction. These are different consumer designs and MUST NOT be conflated.

A partial CDC payload MAY be used as an invalidation signal to build a complete projection from committed authoritative state. Joined or hydrated projections SHOULD reread authoritative state instead of merging unrelated partial changes. Projection mapping and document types stay in the target adapter. Deletion/withdrawal fences MUST survive delayed writes: when physical-delete version memory is finite, use a lasting, versioned tombstone or a fenced fresh-generation rebuild. Readers must exclude tombstones. Deploy compatible readers before tombstone writers and fence older physical-delete writers; see the [projection runbook](durable-worker-runbook.md#projection-fences-and-rebuild).

### 12.4 Replay and failure handling

Each rebuildable projection MUST document its authoritative source, mapping, revision/fence strategy, rebuild/catch-up procedure, and activation checks. Rebuild into an isolated generation where practical, fence old writers, catch up committed changes, and verify identity, versions, and deletion visibility before switching readers. Missing authoritative history cannot be recreated from an expired queue or an existing projection.

On failure, preserve custody, repair the cause, and use small approved replay/redrive with current authoritative-state checks; never purge merely to clear an alarm or log raw source records, credentials, provider payloads, or sensitive job content. Observe lag, backlog/oldest age, failure custody, handler outcomes, duplicates, and projection freshness. Declarations and local tests are not evidence of a live stage; activation and recovery require the owning runbook and approved operator evidence.

### 12.8 Schema evolution

Evolve CDC storage and job contracts with expand-and-contract changes: introduce compatible storage and consumers/readers before enabling new producers or writers; preserve compatibility with retained jobs, replay sources, and rollback binaries before removing old fields. Review routing and projection handlers whenever a CDC source changes. Decoders MUST reject unsupported versions, malformed required fields, or invalid domain identities rather than acknowledge poison input; additive fields MAY be accepted only when the owning wire contract allows it. Event and job formats, selected sources, and receipt schemas belong in [event flow](events/flow.md) and [storage](storage.md), not this guide.

### 12.11 Required tests

CDC and projection tests SHOULD cover valid and invalid source changes, route prevalidation, partial publication and ambiguous acknowledgments, duplicate/concurrent/out-of-order delivery, stale-version fences, and complete-only job acknowledgment. Exercise recovery after lost responses or uncertain external effects, deletion fences, schema compatibility, replay, and a full projection rebuild. Use isolated real infrastructure where needed, but do not mistake local tests or declarations for live activation evidence; real-environment smoke requires separate approval and the owning runbook.

## 13. Error boundaries

Each layer owns its errors.

### Core errors

Domain errors describe business rule failures:

```text
Archived
InvalidTransition
TitleTooLong
NotPermittedByPolicy
```

They MUST NOT contain SQLx, HTTP, or external-client errors.

### Service errors

Use-case errors describe outcomes relevant to callers. Variants MUST name the concrete failure a caller can act on. Avoid catch-all policy or failure variants when the real cause is known.

```text
NotFound
AuthenticatedActorRequired
PlanDoesNotAllowAction
ConcurrencyConflict
ProductListingKeyAlreadyExists
ProductListingDetailsQueryFailed
PersistedProductListingAvailabilityInvalid
```

They MAY wrap internal errors privately but MUST expose stable semantic variants. Do not use vague variants such as `Forbidden`, `Conflict`, `InvalidPersistedState`, or `Internal` when a narrower cause is known.

When a service or port error represents an adapter/read-model failure, keep the original cause as `#[source]` with `application::error::BoxError`. Do not convert technical causes to bare unit variants.

### Adapter errors

Adapter errors describe semantic persistence or integration failures without leaking infrastructure types:

```text
ProductListingLookupByIdFailed
ProductListingInsertFailed
ConcurrencyConflict
ProductListingSlugAlreadyExists
InvalidProductListingUrlPersisted
ExternalResponseMissingPrice
```

They MUST NOT expose SQLx, HTTP-client, or SDK error types in public variants or escape to controllers directly. Preserve the technical cause privately for diagnosis while translating it into a contextual, safe operation error; do not discard the original cause or expose credentials and raw payloads.

### HTTP mapping

Controllers map service errors to HTTP status and response DTOs.

The service MUST NOT return HTTP status codes.

The HTTP boundary owns a consistent, safe problem response and reusable mappings from service errors; transport-owned validation may map directly to an HTTP input error. Public error codes and response fields MUST be stable and documented by tests. Changes to the public contract require updates to [OpenAPI](swagger.yaml) and the [changelog](CHANGELOG.md). Exact error type, constants, and module placement belong in API code.

### Logging errors

Avoid logging the same failure at every layer.

- Adapters add technical context to errors.
- Handlers add use-case context.
- The transport or worker boundary records the terminal failure.
- Expected domain errors SHOULD NOT be logged as infrastructure failures.
- Retries SHOULD emit structured events with attempt and delay fields.

---

## 14. Observability and logging

Use structured `tracing` spans and events.

Every externally invoked use case SHOULD have a span:

```rust
#[tracing::instrument(
    name = "get_record_details",
    skip_all,
    fields(
        record_id = %request.record_id,
        principal_type = context.principal.kind(),
        actor_id = tracing::field::Empty,
        request_id = %context.request_id,
        correlation_id = %context.correlation_id,
    )
)]
```

Record the actor identifier only when the principal has one:

```rust
if let Some(actor_id) = context.principal.actor_id() {
    tracing::Span::current()
        .record("actor_id", tracing::field::display(actor_id));
}
```


### Required context

Where available, spans SHOULD include:

```text
use_case
request_id
correlation_id
principal_type
actor_id, when available
aggregate_id
aggregate_version
data_source
result/outcome
retry_attempt
```

### Sensitive data

Logs and traces MUST NOT contain:

- passwords;
- access tokens;
- session tokens;
- authorization headers;
- secret keys;
- complete payment data;
- sensitive personal content;
- raw request/response bodies by default.

Identifiers MAY be logged only according to the project's privacy policy.

### Adapter spans

Adapters SHOULD create child spans for:

```text
postgres.query
postgres.transaction
search.query
key_value.read
external_source.request
```

Record structured fields such as:

```text
operation
table/index/resource
rows_affected
result_count
duration
timeout
status
```

Do not log parameter values that may contain sensitive data.

### Metrics

At minimum, expose metrics for:

- use-case latency and outcomes;
- database query latency and failures;
- transaction conflicts;
- search latency and result counts;
- external-source latency and failures.

### Operational action logging and audit trail

Relevant state-changing and security-sensitive operations MUST produce structured operational information that identifies:

```text
action
outcome
actor_type
actor_id, when authenticated
target type
target identifier
request_id
correlation_id
resulting version or state, when useful
error category, on failure
```

Examples include creation, publication, permission changes, destructive actions, restoration, authentication changes, and administrative overrides.

Success events for authoritative mutations SHOULD be logged after the transaction commits. This avoids recording a successful action that was later rolled back.

Expected validation or authorization failures SHOULD be recorded with an appropriate outcome and severity, without dumping request payloads or secrets.

Business audit records are not ordinary diagnostic logs. Actions requiring durable, queryable, access-controlled, or legally retained history SHOULD also produce an explicit audit record through the project's audit mechanism. Diagnostic logs alone MUST NOT be treated as a compliance-grade audit trail.

## 15. Authentication, authorization, and operation context

### 15.1 Authentication belongs at the transport boundary

REST authentication is performed by middleware or extractors.

The transport layer validates JWTs or other access tokens and maps validated credentials into a transport principal. Service and core code MUST NOT parse tokens, inspect authorization headers, or depend on JWT/framework types.

The API accepts only explicitly trusted issuers and locally issued credentials; third-party identity-provider tokens are not automatically valid API credentials. A validated delegated credential carries only its granted capabilities. Federated profile attributes MAY initialize application state but MUST NOT overwrite authoritative PostgreSQL-managed state on later provider refreshes. Trusted issuer and front-door routing details belong in the [HTTP front-door contract](http-api-front-door.md).

Protected endpoints SHOULD use an extractor that guarantees an authenticated principal:

```rust
pub(crate) struct AuthenticatedPrincipal {
    pub user_id: UserId,
}
```

Public endpoints SHOULD use an optional principal extractor:

```rust
pub(crate) enum OptionalPrincipal {
    Anonymous,
    User(UserId),
}
```

Rules for public endpoints:

- A missing token maps to anonymous access.
- A valid token maps to its authenticated principal.
- An invalid, expired, malformed, or revoked token MUST be rejected as an authentication failure.
- Invalid supplied credentials MUST NOT be silently downgraded to anonymous access.

The same principle applies to service credentials and system jobs.

Public controllers accepting optional authentication MUST distinguish absent credentials (anonymous) from supplied invalid credentials (authentication error); cover both paths in transport tests.

### 15.2 Service-owned principal model

Controllers map transport principals into service-owned types:

```rust
pub enum Principal {
    Anonymous,
    User(UserId),
    DelegatedUser {
        user_id: UserId,
        capabilities: BTreeSet<CredentialCapability>,
    },
    Service(ServiceId),
    System(SystemActor),
}

pub struct OperationContext {
    pub principal: Principal,
    pub request_id: RequestId,
    pub correlation_id: CorrelationId,
}
```

Framework-specific authentication objects MUST NOT cross the transport boundary.

`aura-historia-api` creates server-side request IDs and accepts correlation IDs only from the configured transport metadata source. Controllers map the transport principal plus request metadata into `OperationContext` before invoking a use case.

Externally invoked use cases SHOULD receive `&OperationContext` separately from their command or query request:

```rust
use_case.execute(&context, command).await
```

The command/request contains business input. The operation context contains caller and request metadata.

This separation prevents trusted identity from being accepted from JSON bodies, query parameters, or path values.

### 15.3 Protected commands

A command that changes protected state MUST require an authenticated actor or an explicitly permitted service/system principal.

Do not model required authentication as `Option<UserId>`:

```rust
// Forbidden for a protected mutation
pub actor_id: Option<UserId>
```

Instead, require the principal at the use-case boundary:

```rust
let actor = context
    .actor_label()
    .ok_or(RenameRecordError::AuthenticatedActorRequired)?;
```

A controller route being protected is not sufficient authorization. The use case MUST enforce authorization or invoke a service/domain policy.

### 15.4 Public queries

A public query such as `GetRecordDetails` MAY accept anonymous callers:

```rust
match &context.principal {
    Principal::Anonymous => {
        // Return public fields only.
    }
    Principal::User(user_id) => {
        // Optionally hydrate user-specific state.
    }
    Principal::Service(service_id) => {
        // Apply the documented service policy.
    }
    Principal::System(actor) => {
        // Apply the documented internal policy.
    }
}
```

Public availability and authenticated personalization are separate concerns.

A public query SHOULD NOT require identity when no authorization, personalization, rate policy, or operational requirement uses it. It MAY still receive `OperationContext` for consistent request correlation and actor-aware logging.

### 15.5 Authorization

Authorization belongs to the use case or an explicit service/domain policy.

Examples:

```text
CanRenameRecord
CanDeleteRecord
CanViewPrivateFields
CanActForWorkspace
```

Authorization MUST use trusted identity from `OperationContext`, never a user identifier supplied by the request body.

Domain methods MAY receive an already-resolved permission or policy value when authorization affects a domain invariant.

### 15.6 Credential scopes

Scopes belong to delegated Aura Historia access tokens, not Cognito JWTs. Cognito-authenticated users, service principals, and system principals use an open-world assumption for credential capability checks; business constraints such as admin role, same-user access, partnership membership, or ownership still MUST be enforced separately by use cases or service policies.

Aura Historia access tokens use a closed-world assumption: a delegated principal has only the scopes stored on the token. Use cases that protect a state change or private read MUST check the narrowest matching `CredentialCapability` before executing protected work.

Scope names SHOULD use `resource:action` with plural resources and stable actions; the exact registry is owned by the credential contract and public API. Prefer `read`, `write`, or an explicit non-role action over vague scopes such as `records:manage`. Roles are not scopes: admin-only use cases MUST check the relevant role or policy after credential capability checks. Public queries SHOULD NOT require scopes unless authenticated state changes the returned private data or policy.

### 15.7 Operational identity logging

Every externally invoked use case SHOULD have a structured tracing span containing:

```text
principal type
actor identifier, when available
request identifier
correlation identifier
use-case name
target identifier, when available
outcome
```

Anonymous calls MUST be recorded as anonymous, not with a fabricated user identifier.

For relevant mutations such as deletion, publication, permission changes, or administrative actions, the committed success event MUST identify who performed the action:

```rust
tracing::info!(
    event = "record.deleted",
    actor_type = actor.kind(),
    actor_id = %actor.id(),
    record_id = %record_id,
    outcome = "success",
);
```

Access tokens, JWT claims as raw JSON, and authorization headers MUST NOT be logged.

## 16. Configuration and secrets

Configuration is loaded at the composition root.

Adapters receive typed configuration through constructors.

```rust
pub(crate) struct SearchConfig {
    pub endpoint: Url,
}
```

`core` and `service` MUST NOT read environment variables.

### Outbound network destinations

Adapters that fetch untrusted or externally supplied URLs MUST enforce an SSRF policy at the network boundary. Validate URL syntax and every redirect, resolve DNS safely, exclude unsafe/special-use addresses, and pin approved peer addresses for the actual connection. Address resolution or classification failures MUST fail closed; address classification alone does not prove a request will reach an approved destination.

Secrets MUST NOT be embedded in domain/application types, logs, errors, or committed configuration files.

External clients and pools SHOULD be constructed at the composition boundary and reused rather than created per request.

---

## 17. Concurrency and idempotency

### Optimistic concurrency

Aggregate tables SHOULD contain a monotonically increasing version. Updates MUST compare the loaded version and advance it once for a changed write.

No returned row MUST map to an internal concurrency-conflict error when the row was expected to exist. Do not leak the concrete version value in errors. The returned version is authoritative only for PostgreSQL internals and CDC consumers; ordinary use cases SHOULD NOT return it.

A reconciliation that changes aggregates must lock its owner first, then affected aggregate rows in a stable order. Each changed row increments its version, so a stale ordinary aggregate write fails rather than restoring old state.

### Idempotency

Externally retried commands SHOULD accept an idempotency key when duplicate execution would be harmful.

Idempotency handling belongs to the use-case transaction boundary.

The result of a completed idempotent command SHOULD be replayable without repeating its side effects.


## 18. API controller rules

A controller owns:

- REST path/query/header extraction;
- authentication principal extraction;
- mapping transport identity into `OperationContext`;
- request DTOs;
- response DTOs;
- transport-level validation;
- request-to-use-case mapping;
- use-case invocation;
- use-case-result-to-response mapping;
- cache and representation headers owned by the public REST contract;
- service-error-to-HTTP mapping through crate error mappings.

A controller MUST NOT:

- access a database client;
- call a repository;
- build SQL or search DSL;
- compose multiple data sources;
- construct concrete adapters;
- enforce business authorization policy such as admin role, ownership, or partnership relation;
- enforce domain invariants;
- mutate aggregates;
- return storage types;
- decide transaction scope.

Canonical flow:

```text
HTTP request
    -> axum extractor
    -> transport auth principal
    -> OperationContext
    -> REST request DTO/path/query values
    -> service command/request
    -> use-case trait from AppState
    -> service result/view
    -> REST response DTO
    -> HTTP response or ApiError problem JSON
```

Transport state SHOULD expose inbound use cases and authenticators, not repositories or infrastructure clients. Route modules remain thin.

For read endpoints with cache behavior, cache headers are REST contract and belong in the controller. If the cache policy depends on anonymous vs authenticated access, derive it from `OperationContext.principal`, not from raw headers.

Query use cases SHOULD be read-optimized for their public result shape. If an API needs a summary list, the query use case should return summary read models directly instead of returning IDs for controller-side hydration. Controllers MUST NOT introduce N+1 reads to assemble response payloads.

Command use cases SHOULD return the public command result/view directly from their write model. Controllers MUST NOT perform a follow-up read after `create`, `update`, or similar writes to assemble the response.

---

## 19. Use-case dispatch

Inbound use-case contracts MUST be usable through the runtime's chosen dispatch mechanism (often trait objects). Outbound ports MAY use static dispatch where practical. Async-trait syntax and object-safety mechanics belong to code and the workspace toolchain; do not let dispatch choices reverse dependency direction.

---

## 20. Testing strategy

Test placement follows visibility and architectural intent. Private implementation tests belong beside the implementation; black-box tests under `tests/` exercise the deliberate public API. Do not widen production visibility merely for a test, even one using real infrastructure. Shared test fixtures and runners are code-level details, not architecture rules.

- Core tests verify invariants, transitions, and no-op/event behavior without infrastructure.
- Service tests use fakes or mocks for ports to verify orchestration, authorization, transaction/commit behavior, batching, error mapping, and optional-data policy.
- Adapter tests verify fallible mapping, persisted-state validation, SQL concurrency/rollback against real PostgreSQL where appropriate, provider wire/error behavior, and stale projection fences.
- Transport tests invoke the public boundary with fake inbound use cases to cover authentication, validation, DTO/error mapping, and response contract. Black-box acceptance tests cover critical behavior through the exposed API with isolated real dependencies when needed.
- Durable delivery tests cover duplicate, out-of-order, lost-response, partial-publish, and uncertain external-effect paths. An integration or local test is not evidence of deployment or live-service acceptance; use the owning runbook for operational verification.

## 21. Naming conventions

Use these suffixes consistently:

| Suffix | Meaning |
|---|---|
| `...Command` | Write use-case input |
| `...Request` | Read use-case input |
| `...Result` | Write result or paginated query result |
| `...View` | Final or partial application read model |
| `...Summary` | Compact application read model |
| `...UseCase` | Controller-facing inbound trait |
| `...Handler` | Focused use-case implementation |
| `...Repository` | Aggregate reconstruction/persistence |
| `...Reader` | Purpose-specific read capability |
| `...Policy` | Domain or application decision abstraction |
| `...Row` | PostgreSQL row representation |
| `...Document` | Search document representation |

| `...Record` | External/graph/source response representation |
| `...Dto` | Transport representation |
| `...Event` | Domain event |
| `-core` | Entity domain crate |
| `-service` | Entity application/use-case crate |
| `-postgres` | Entity PostgreSQL adapter crate |
| `-opensearch` | Entity OpenSearch adapter crate |


Avoid vague names:

```text
Manager
Helper
Util
Common
GenericRepository
DataService
DatabaseService
OpenSearchQuery
PostgresPort
```

Names SHOULD describe business intent or application capability. When bounded contexts have similarly shaped lifecycle values, keep their Rust names distinct (`SearchFilterState`, `WatchlistState`) instead of using a generic compatibility alias.

---

## 22. Forbidden patterns

The following patterns MUST NOT be introduced without an approved architecture change:

### Generic cross-store repository

```rust
trait Repository<T, Id> {
    async fn save(&self, value: T);
    async fn find(&self, id: Id);
    async fn delete(&self, id: Id);
}
```

### Repository used for presentation reads

```rust
record_repository.search_with_container_and_user_state(...)
```

### Storage type escaping adapter

```rust
fn controller(...) -> Json<RecordRow>
```

### Controller orchestration

```rust
let hits = search_client.search(...).await?;
let states = postgres_reader.read(...).await?;
let result = merge(hits, states);
```

### N+1 hydration

```rust
for hit in hits {
    user_state_reader.find_one(actor_id, hit.id).await?;
}
```

### Domain depending on infrastructure

```rust
#[derive(sqlx::FromRow)]
pub struct Record { ... }
```

### One god service

```rust
struct RecordService {
    repository: ...,
    search: ...,
    metadata: ...,
    user_state: ...,
    queue: ...,
    cache: ...,
    // dependencies for every use case
}
```

### Hidden distributed transaction

```text
BEGIN PostgreSQL
update PostgreSQL
update search index
update key-value store
COMMIT PostgreSQL
```

### Logging sensitive payloads

```rust
tracing::info!(?request_body, authorization = %header);
```

### Silent persisted-state corruption

```rust
impl From<RecordRow> for Record {
    // unchecked construction that bypasses invariants
}
```

---

## 23. Design and review checklist

Before an architecture-affecting change, identify the bounded context and operational source of truth; assign each type, use case, port, mapping, and transaction to its owner. Review whether authorization and required reads use trusted identity and an appropriately consistent snapshot; whether cross-system calls can fail or complete ambiguously; and whether retries, provider limits, privacy, and retention need a documented contract. Keep adapters and controllers thin, limit public visibility, and verify no N+1 reads or hidden distributed transactions were introduced.

Tests SHOULD demonstrate domain invariants, service orchestration, adapter mapping/concurrency, transport contracts, and relevant delivery/recovery behavior. Update the appropriate specialized contract when changing wire, storage, deployment, or operations; amend this guide only for durable general design rules. Explain intentional architectural deviations in the pull request.
