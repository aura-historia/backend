# Architecture Guide

This document defines the default architecture and implementation conventions for this workspace. It is intended to be read before adding or changing domain logic, use cases, persistence, APIs, integrations, or projections.

Changes that intentionally deviate from this document MUST explain the reason in the pull request and SHOULD update this document when the deviation represents a new general rule.

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
read handlers and orchestration
outbound capability ports
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
2. Use-case contracts and their input/output types belong to `service`.
3. Domain behavior belongs to `core`.
4. Ports describe application capabilities, not databases.
5. Adapters implement ports.
6. Storage representations remain private to their adapters.
7. Repositories reconstruct and persist aggregates.
8. Readers build read models.
9. Read models are not aggregates.
10. Write handlers define transaction scope.
11. PostgreSQL-aware write handlers explicitly begin and commit SQLx transactions.
12. Cross-datasource writes do not share a transaction.
13. Projection and CDC behavior follows the dedicated CDC architecture documentation.
14. REST DTOs belong to the REST layer and are mapped by controllers.
15. Trusted caller identity is mapped into a service-owned `OperationContext`.
16. Domain and service code MUST NOT depend on infrastructure types.

---

## 3. Canonical module layout

A domain/feature crate SHOULD use this structure:

```text
crates/record/
└── src/
    ├── lib.rs
    │
    ├── core/
    │   ├── mod.rs
    │   ├── record.rs
    │   ├── record_id.rs
    │   ├── workspace_id.rs
    │   ├── value_objects.rs
    │   ├── events.rs              # optional
    │   ├── policies.rs
    │   └── errors.rs
    │
    ├── service/
    │   ├── mod.rs
    │   ├── use_cases/
    │   │   ├── mod.rs
    │   │   ├── commands/
    │   │   │   ├── mod.rs
    │   │   │   ├── create_record.rs
    │   │   │   ├── rename_record.rs
    │   │   │   └── archive_record.rs
    │   │   └── queries/
    │   │       ├── mod.rs
    │   │       ├── search_records.rs
    │   │       └── get_record_details.rs
    │   │
    │   ├── ports/
    │   │   ├── mod.rs
    │   │   ├── record_repository.rs
    │   │   ├── record_search_reader.rs
    │   │   ├── record_details_reader.rs
    │   │   ├── record_user_state_reader.rs
    │   │   └── record_metadata_reader.rs
    │   │
    │   └── use_case_bundle.rs
    │
    ├── postgres/
    │   ├── mod.rs
    │   ├── use_cases/
    │   │   ├── mod.rs
    │   │   ├── create_record.rs
    │   │   └── rename_record.rs
    │   ├── repositories/
    │   │   ├── mod.rs
    │   │   └── record_repository.rs
    │   ├── readers/
    │   │   ├── mod.rs
    │   │   ├── record_details_reader.rs
    │   │   └── record_user_state_reader.rs
    │   ├── rows/
    │   │   ├── mod.rs
    │   │   ├── record_row.rs
    │   │   └── record_details_row.rs
    │   └── mapping.rs
    │
    ├── search/
    │   ├── mod.rs
    │   ├── record_document.rs
    │   ├── record_search_reader.rs
    │   └── projector.rs
    │
    ├── key_value/
    │   ├── mod.rs
    │   ├── record_item.rs
    │   ├── record_reader.rs
    │   └── projector.rs
    │
    ├── additional_source/
    │   ├── mod.rs
    │   ├── record.rs
    │   └── record_metadata_reader.rs
```

The example names `search`, `key_value`, and `additional_source` describe adapter roles. Actual modules MAY use technology names when that improves discoverability.

REST code lives outside the domain/feature crate:

```text
crates/api/
└── src/
    ├── record/
    │   ├── controller.rs
    │   ├── request_dto.rs
    │   ├── response_dto.rs
    │   └── mapping.rs
    └── router.rs
```

### Dependency direction

```text
core
  ▲
  │
service
  ▲
  │
postgres / search / key_value / additional_source
  ▲
  │
composition root and transport
```

Allowed dependencies:

```text
service             -> core
postgres            -> service + core
search              -> service + core identifiers as needed
key_value           -> service + core identifiers as needed
additional_source   -> service + core identifiers as needed
api                 -> service + public core identifiers/value objects as needed
```

Forbidden dependencies:

```text
core                -X-> service
core                -X-> adapters
service             -X-> adapters
service             -X-> REST DTOs
adapter A           -X-> private types from adapter B
controller          -X-> concrete database client
controller          -X-> repository
controller          -X-> storage row/document/item
```

---

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

Operational metadata such as `created_at`, `updated_at`, `created_by`, and `updated_by` is not aggregate state unless a domain invariant explicitly depends on it.

It MUST NOT be added to an aggregate merely for audit, display, sorting, or transport compatibility.

This metadata lives in the persistence layer. The repository owns writing and updating it from the use-case `OperationContext`, clocks, and persistence defaults. When reconstructing an aggregate, the repository MUST map only domain state back into the aggregate.

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


## 5. Type ownership and visibility

### 5.1 Ownership table

| Type category | Example | Owner | Default visibility |
|---|---|---|---|
| Aggregate | `Record` | `core` | `pub`, fields private |
| Value object | `RecordTitle` | `core` | `pub`, fields private |
| Typed ID | `RecordId` | `core` or shared identifiers crate | `pub` |
| Domain event, when used | `RecordEvent` | `core` | `pub(crate)` or `pub` when required |
| Principal/context | `Principal`, `OperationContext` | `service` | `pub` |
| Use-case command | `RenameRecordCommand` | `service/use_cases` | `pub` |
| Query request | `SearchRecordsRequest` | `service/use_cases` | `pub` |
| Use-case result | `RenameRecordResult` | `service/use_cases` | `pub` |
| Read model/view | `RecordSummary` | `service/use_cases` | `pub` |
| Inbound use-case trait | `RenameRecordUseCase` | `service/use_cases` | `pub` |
| Outbound port | `RecordSearchReader` | `service/ports` | `pub(crate)` by default |
| Read handler | `SearchRecordsHandler` | `service/use_cases` | `pub(crate)` |
| PostgreSQL write handler | `PostgresRenameRecordHandler` | `postgres/use_cases` | `pub(crate)` |
| PostgreSQL row | `RecordRow` | `postgres/rows` | private or `pub(super)` |
| Search document | `RecordDocument` | search adapter | private or `pub(super)` |
| Key-value item | `RecordItem` | key-value adapter | private or `pub(super)` |
| External-source record | `ExternalRecord` | source adapter | private or `pub(super)` |
| REST request DTO | `RenameRecordRequestDto` | API controller module | `pub(crate)` |
| REST response DTO | `RecordDetailsResponseDto` | API controller module | `pub(crate)` |
| Use-case bundle | `RecordUseCases` | `service` | `pub` |
| Public builder | `build_record_use_cases` | `wiring` | `pub` |

### 5.2 Visibility rules

Use the narrowest visibility that works.

- Aggregate fields MUST be private.
- Storage fields SHOULD be private to the adapter.
- Storage types SHOULD be private when used in one file.
- Use `pub(super)` when a parent adapter module needs the type.
- Use `pub(crate)` for cross-module implementation details inside one crate.
- Use `pub` only for deliberate crate API.
- Outbound ports SHOULD be `pub(crate)` while all adapters are modules in the same crate.
- If adapters become separate crates, the required ports MUST be promoted to `pub`.
- Concrete adapters SHOULD remain `pub(crate)`.
- Integration tests in `tests/` MUST NOT make adapters public just for tests; expose a narrow `#[cfg(feature = "test-data")]` test helper/facade when private adapter visibility blocks repository tests.
- Prefer a public builder returning public use-case trait objects over exposing concrete implementations.

Example:

```rust
// Public contract used by the API crate.
#[async_trait::async_trait]
pub trait RenameRecordUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: RenameRecordCommand,
    ) -> Result<RenameRecordResult, RenameRecordError>;
}

// Internal implementation.
pub(crate) struct PostgresRenameRecordHandler {
    pool: sqlx::PgPool,
}
```

Public wiring:

```rust
pub struct RecordUseCases {
    pub rename: Arc<dyn RenameRecordUseCase>,
    pub search: Arc<dyn SearchRecordsUseCase>,
    pub details: Arc<dyn GetRecordDetailsUseCase>,
}

pub fn build_record_use_cases(
    dependencies: RecordDependencies,
) -> RecordUseCases {
    // Construct concrete handlers internally.
    todo!()
}
```

`RecordUseCases` is a dependency container, not a service façade. It MUST NOT contain forwarding business logic.

---

## 6. Use cases

Reads and writes are both use cases.

Each use case SHOULD have its own file. The file owns:

- command or request;
- result or final view;
- use-case error;
- inbound use-case trait;
- datastore-independent handler when applicable.

### 6.1 Write use-case contract

```rust
// service/use_cases/commands/rename_record.rs

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

When a public PATCH endpoint is intentionally broad, the service use case is still named `Update*`. Use a service-owned `Update*Command` with shared tri-state `common::patch_field::PatchField`: `Unchanged`, `Set(value)`, and `Clear`. The helper may live in `common`, but the command belongs in `service`, not in `core`.

The update handler MUST translate command fields into explicit aggregate methods such as `change_title`, `replace_address`, or `replace_contact`, and track `ChangeOutcome`. Generic update MUST NOT include state-machine transitions that deserve their own use case, such as publishing, archiving, or changing aggregate status.

A broad update use case is one logical write. Domain methods return `ChangeOutcome` and MUST NOT know about storage versions. If nothing changed, the handler MUST NOT execute a persistence update. If anything changed, the handler calls repository `update(&aggregate, loaded.version)` once. The PostgreSQL repository writes complete authoritative state, enforces optimistic concurrency with `WHERE version = $expected_version`, and increments `version` exactly once with `version = version + 1`. The new version is internal PostgreSQL/CDC state and MUST NOT be returned from ordinary use cases.

### 6.2 Read use-case contract

```rust
// service/use_cases/queries/search_records.rs

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

A use case SHOULD have one focused implementation.

Preferred:

```text
RenameRecordUseCase
    implemented by PostgresRenameRecordHandler

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

### 6.4 Handler location

A handler that depends only on abstract ports belongs in `service/use_cases`.

Example:

```text
service/use_cases/queries/search_records.rs
    SearchRecordsHandler
        -> RecordSearchReader
        -> RecordUserStateReader
```

A handler that directly creates an SQLx transaction or concrete PostgreSQL repositories belongs in:

```text
postgres/use_cases/
```

It still implements the service-owned use-case trait.

This is the project convention for write handlers because it keeps SQLx out of `service` while retaining simple transaction code.

---

## 7. Inbound use-case traits and outbound ports

There are two directions of traits.

```text
REST/controller
    │
    │ calls inbound port
    ▼
RenameRecordUseCase
    │
    │ implemented by handler
    ▼
PostgresRenameRecordHandler
    │
    │ calls outbound port / adapter
    ▼
RecordRepository
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

They belong to `service/use_cases`.

Controllers MUST depend on inbound use-case traits, never concrete adapters.

### Outbound ports

Outbound ports describe capabilities needed by handlers.

Examples:

```text
RecordRepository
RecordSearchReader
RecordDetailsReader
RecordUserStateReader
RecordMetadataReader
Clock
AuthorizationPolicy
IdempotencyStore
```

They belong to `service/ports`.

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
DynamoPort
ExternalDatabasePort
```

One adapter may implement multiple ports. One port may have multiple implementations.

```text
RecordDetailsReader
    <- PostgresRecordDetailsReader
    <- KeyValueRecordDetailsReader

RecordSearchReader
    <- SearchEngineRecordSearchReader
    <- PostgresRecordSearchReader
```

There is not one port per data source.

---

## 8. Repositories

### 8.1 Responsibility

A repository reconstructs and persists an aggregate.

```rust
#[async_trait::async_trait]
type VersionedRecord = Versioned<Record, RecordStorageVersion>;

pub(crate) trait RecordRepository {
    async fn find_by_id(
        &mut self,
        id: RecordId,
    ) -> Result<Option<VersionedRecord>, RecordRepositoryError>;

    async fn insert(
        &mut self,
        record: &Record,
    ) -> Result<(), RecordRepositoryError>;

    async fn update(
        &mut self,
        record: &Record,
        expected_version: RecordStorageVersion,
    ) -> Result<(), RecordRepositoryError>;
}
```

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

`insert` means the aggregate MUST be new.

`update` means the aggregate MUST exist and MUST enforce optimistic concurrency through an internal loaded storage version.

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
pub(crate) trait RecordDetailsReader: Send + Sync {
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
pub(crate) trait RecordSearchReader: Send + Sync {
    async fn search(
        &self,
        request: &SearchRecordsRequest,
    ) -> Result<SearchResult<RecordSearchHit>, RecordSearchReadError>;
}

#[async_trait::async_trait]
pub(crate) trait RecordUserStateReader: Send + Sync {
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

The handler owns the final result:

```rust
pub struct RecordDetailsView {
    pub record_id: RecordId,
    pub title: String,
    pub container: ContainerSummary,
    pub metadata: MetadataSection,
    pub user_state: Option<RecordUserState>,
}
```

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

Mapping code belongs at the boundary that owns the source representation.

```text
REST DTO              -> controller/API mapping
PostgreSQL row        -> PostgreSQL adapter mapping
Search document       -> search adapter mapping
Key-value item        -> key-value adapter mapping
External response     -> corresponding adapter mapping
```

Storage and transport mapping MUST NOT be placed in `core`.

### 10.1 REST mapping

REST request and response DTOs belong to the controller/API module.

```rust
#[derive(serde::Deserialize)]
pub(crate) struct RenameRecordRequestDto {
    pub title: String,
}

#[derive(serde::Serialize)]
pub(crate) struct RenameRecordResponseDto {
    pub id: Uuid,
    pub title: String,
}
```

The controller maps request DTOs into service-owned commands.

Use `TryFrom` when parsing or validation can fail:

```rust
impl TryFrom<(RecordId, RenameRecordRequestDto)>
    for RenameRecordCommand
{
    type Error = ApiInputError;

    fn try_from(
        value: (RecordId, RenameRecordRequestDto),
    ) -> Result<Self, Self::Error> {
        let (record_id, dto) = value;

        Ok(Self {
            record_id,
            new_title: dto.title,
        })
    }
}
```

Use `From` for infallible result-to-response conversion:

```rust
impl From<RenameRecordResult> for RenameRecordResponseDto {
    fn from(result: RenameRecordResult) -> Self {
        Self {
            id: result.record_id.into_uuid(),
            title: result.title,
        }
    }
}
```

The mapping implementation SHOULD live in:

```text
api/record/mapping.rs
```

Small mappings used by only one controller MAY live in the controller file.

The service MUST NOT know REST DTOs or HTTP status codes.

### 10.2 PostgreSQL deserialization with `FromRow`

PostgreSQL rows SHOULD use `sqlx::FromRow` for deserialization.

```rust
// postgres/rows/record_row.rs

#[derive(Debug, sqlx::FromRow)]
pub(super) struct RecordRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub title: String,
    pub status: String,
    pub version: i64,
}
```

`RecordRow` is a storage representation. It MUST remain private to the PostgreSQL adapter.

Deserialization flow:

```text
PostgreSQL row
    -> sqlx::FromRow
RecordRow
    -> TryFrom / adapter mapping
Record
```

Mapping to the aggregate SHOULD use `TryFrom` because persisted state may be corrupt or incompatible:

```rust
// postgres/mapping.rs

impl TryFrom<RecordRow> for Versioned<Record, RecordStorageVersion> {
    type Error = RecordRowMappingError;

    fn try_from(row: RecordRow) -> Result<Self, Self::Error> {
        let version = RecordStorageVersion::try_from(row.version)?;
        let record = Record::rehydrate(
            RecordId::from_uuid(row.id),
            WorkspaceId::from_uuid(row.workspace_id),
            RecordTitle::try_from(row.title)?,
            RecordStatus::try_from(row.status.as_str())?,
        )
        .map_err(RecordRowMappingError::InvalidPersistedState)?;

        Ok(Versioned::new(record, version))
    }
}
```

The domain MAY expose a crate-visible rehydration function:

```rust
impl Record {
    pub(crate) fn rehydrate(
        id: RecordId,
        workspace_id: WorkspaceId,
        title: RecordTitle,
        status: RecordStatus,
    ) -> Result<Self, RehydrateRecordError> {
        // Re-establish invariants without emitting new events.
        todo!()
    }
}
```

`rehydrate` MUST:

- establish all required invariants;
- not emit new domain events;
- not pretend persisted data is automatically valid.

A joined query gets its own row type:

```rust
#[derive(Debug, sqlx::FromRow)]
struct RecordDetailsRow {
    record_id: Uuid,
    record_title: String,
    container_id: Uuid,
    container_name: String,
}
```

It maps directly to the application read model:

```rust
impl From<RecordDetailsRow> for RecordBaseDetails {
    fn from(row: RecordDetailsRow) -> Self {
        Self {
            record_id: RecordId::from_uuid(row.record_id),
            title: row.record_title,
            container: ContainerSummary {
                container_id: ContainerId::from_uuid(row.container_id),
                name: row.container_name,
            },
        }
    }
}
```

Use `TryFrom` instead of `From` when mapping can fail.

### 10.3 PostgreSQL serialization

Serialization from a domain aggregate to PostgreSQL MUST occur inside the concrete repository/DAO implementation.

Preferred:

```rust
impl SqlxRecordRepository<'_> {
    async fn update(
        &mut self,
        record: &Record,
        expected_version: RecordStorageVersion,
    ) -> Result<(), RecordRepositoryError> {
        let row = sqlx::query_as::<_, (i64,)>(
            r#"
            UPDATE records
            SET
                title = $1,
                status = $2,
                version = version + 1,
                updated_at = now()
            WHERE id = $3
              AND version = $4
            RETURNING version
            "#,
        )
        .bind(record.title().as_str())
        .bind(record.status().as_db_str())
        .bind(record.id().as_uuid())
        .bind(expected_version.into_inner())
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(RecordRepositoryError::from)?;

        let Some((version,)) = row else {
            return Err(RecordRepositoryError::ConcurrencyConflict);
        };

        RecordStorageVersion::try_from(version)
            .map_err(|_| RecordRepositoryError::InvalidPersistedState)?;

        Ok(())
    }
}
```

The project SHOULD NOT define a public or cross-layer conversion such as:

```rust
impl From<&Record> for RecordRow
```

for write serialization.

Reasons:

- write parameters may differ from read rows;
- inserts and updates need different fields;
- database-specific encoding belongs to the DAO;
- a generic row conversion can conceal optimistic concurrency and generated columns.

When binding logic is repeated, use a private adapter-local helper:

```rust
struct RecordWriteParams<'a> {
    title: &'a str,
    status: &'a str,
    version: i64,
}
```

That helper MUST remain private to the PostgreSQL adapter.

### 10.4 Search document mapping

A search adapter owns its document:

```rust
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct RecordDocument {
    id: String,
    title: String,
    container_name: String,
    projection_version: u64,
}
```

Read mapping:

```text
Search response JSON
    -> RecordDocument
    -> RecordSearchHit
```

Mapping used to build or update projections belongs in the source adapter and follows the CDC architecture documented in Section 12.

The search document MUST NOT escape the adapter.

### 10.5 External response mapping

An adapter for an additional data source owns the external response type.

```rust
#[derive(serde::Deserialize)]
struct ExternalMetadataResponse {
    labels: Vec<String>,
    relations: Vec<ExternalRelation>,
}
```

It maps to an application-owned model:

```rust
pub struct RecordMetadataView {
    pub labels: Vec<String>,
    pub relations: Vec<MetadataRelation>,
}
```

External-client types MUST NOT appear in service contracts.

---

## 11. Transactions

### 11.1 Ownership

The use-case handler defines the transaction scope.

Because this project intentionally uses SQLx directly for PostgreSQL write orchestration, the concrete PostgreSQL write handler:

1. begins the transaction;
2. constructs transaction-scoped repositories/readers;
3. executes domain behavior;
4. writes all authoritative state required by the use case;
5. explicitly commits.

The service module owns the use-case contract. The PostgreSQL module owns the SQLx-aware implementation.

```text
service/use_cases/commands/rename_record.rs
    RenameRecordCommand
    RenameRecordResult
    RenameRecordError
    RenameRecordUseCase trait

postgres/use_cases/rename_record.rs
    PostgresRenameRecordHandler
    SQLx transaction scope
```

### 11.2 Transaction-scoped repository

```rust
pub(crate) struct SqlxRecordRepository<'tx> {
    connection: &'tx mut sqlx::PgConnection,
}

impl<'tx> SqlxRecordRepository<'tx> {
    pub(crate) fn new(
        connection: &'tx mut sqlx::PgConnection,
    ) -> Self {
        Self { connection }
    }
}
```

The repository implements the service-owned repository port.

### 11.3 Chained temporary repositories

For one operation, construct and call the repository in one chain:

```rust
let mut record = SqlxRecordRepository::new(&mut *tx)
    .find_by_id(command.record_id)
    .await?
    .ok_or(RenameRecordError::NotFound)?;
```

The temporary repository is dropped at the semicolon, releasing its mutable borrow of the transaction.

Subsequent repositories can borrow the same transaction:

```rust
let workspace = SqlxWorkspaceRepository::new(&mut *tx)
    .find_by_id(record.workspace_id())
    .await?
    .ok_or(RenameRecordError::WorkspaceNotFound)?;
```

Writes follow the same pattern:

```rust
SqlxRecordRepository::new(&mut *tx)
    .update(&record, loaded_version)
    .await?;
```

### 11.4 Canonical PostgreSQL write handler

```rust
#[async_trait::async_trait]
impl RenameRecordUseCase for PostgresRenameRecordHandler {
    #[tracing::instrument(
        name = "rename_record",
        skip_all,
        fields(
            record_id = %command.record_id,
            actor_type = tracing::field::Empty,
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: RenameRecordCommand,
    ) -> Result<RenameRecordResult, RenameRecordError> {
        let actor = context
            .actor_label()
            .ok_or(RenameRecordError::AuthenticatedActorRequired)?;

        tracing::Span::current().record("actor_type", context.principal.kind());
        tracing::Span::current()
            .record("actor_id", tracing::field::display(&actor));

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(RenameRecordError::from)?;

        let loaded = SqlxRecordRepository::new(&mut *tx)
            .find_by_id(command.record_id)
            .await?
            .ok_or(RenameRecordError::NotFound)?;
        let loaded_version = loaded.version;
        let mut record = loaded.value;

        authorize_rename(actor, &record)?;

        let new_title = RecordTitle::try_from(command.new_title)
            .map_err(|_| RenameRecordError::InvalidTitle)?;

        record.rename(new_title)?;

        SqlxRecordRepository::new(&mut *tx)
            .update(&record, loaded_version)
            .await?;

        tx.commit()
            .await
            .map_err(RenameRecordError::from)?;

        tracing::info!(
            event = "record.renamed",
            actor_type = actor.kind(),
            actor_id = %actor.id(),
            record_id = %record.id(),
            outcome = "success",
        );

        Ok(RenameRecordResult {
            record_id: record.id(),
            title: record.title().to_string(),
        })
    }
}
```

A successful transaction MUST end in explicit `commit().await`.

An uncommitted transaction that leaves scope is expected to roll back. Code MUST NOT treat dropping or “closing” the transaction as commit.

Operational success events for state changes SHOULD be emitted only after the authoritative transaction commits. Failed attempts MAY be logged at the terminal boundary with a safe outcome and error category.

### 11.5 Cross-entity transactions

When two aggregates/tables are in the same PostgreSQL database and the use case requires atomicity, the handler MAY use multiple repositories against the same transaction.

```rust
let mut tx = self.pool.begin().await?;

let mut record = SqlxRecordRepository::new(&mut *tx)
    .find_by_id(command.record_id)
    .await?
    .ok_or(Error::RecordNotFound)?;

let workspace = SqlxWorkspaceRepository::new(&mut *tx)
    .find_by_id(record.workspace_id())
    .await?
    .ok_or(Error::WorkspaceNotFound)?;

record.activate(workspace.activation_policy())?;

SqlxRecordRepository::new(&mut *tx)
    .update(&record, loaded_version)
    .await?;

tx.commit().await?;
```

Cross-entity atomicity MUST NOT be confused with cross-datasource atomicity.

### 11.6 Readers inside a write transaction

A read that influences an invariant-critical write MUST use the same transaction.

```rust
let policy = SqlxWorkspacePolicyReader::new(&mut *tx)
    .find_policy(record.workspace_id())
    .await?;
```

Do not call a pool-backed reader on another connection when the result must be consistent with the active write transaction.

### 11.7 Ordinary readers

Ordinary presentation reads SHOULD own a pool/client and SHOULD NOT receive an explicit transaction.

```rust
pub(crate) struct PostgresRecordDetailsReader {
    pool: sqlx::PgPool,
}
```

A single SQL statement does not need an application-managed transaction.

Use an explicit read transaction only when several SQL statements must observe one consistent snapshot. This is exceptional and SHOULD be documented in the use case.

### 11.8 Cross-datasource boundaries

A PostgreSQL transaction cannot atomically include:

- a search engine;
- a key-value store;
- a graph or knowledge source;
- an external API;
- a message broker without a specific transaction protocol.

Authoritative PostgreSQL writes MUST commit according to the transaction rules above. Replication and projection propagation follow the dedicated CDC architecture documentation.

---

## 12. CDC and projection architecture

CDC propagates committed PostgreSQL changes to workers and rebuildable read projections.

```text
PostgreSQL commit
    -> logical replication
    -> Sequin
    -> CDC router
    -> bounded worker queues
    -> projection handlers
```

### 12.1 Storage ownership

Every dataset MUST have one documented operational owner.

PostgreSQL owns business truth for:

* users;
* shops;
* partner-shop applications;
* products;
* product events;
* product FX snapshots plus EUR-based conversion rows;
* product translations;
* product watchlists;
* search filters;
* search-filter matches.

DynamoDB remains the operational owner for:

* notifications with TTL and insert-to-send behavior;
* access tokens;
* OAuth clients;
* OAuth authorization codes;
* OAuth third-party exchange codes.

OpenSearch contains rebuildable search projections only.

Additional stores MUST be classified as one of:

* authoritative operational storage;
* rebuildable projection;
* external source;
* cache.

A store MUST NOT become authoritative merely because it is used on a hot read path.

### 12.2 Write contract

Authoritative PostgreSQL writes occur in one SQLx transaction.

External projections MUST NOT be updated synchronously inside that transaction.

Forbidden:

```text
BEGIN PostgreSQL
update PostgreSQL
update OpenSearch
update key-value projection
COMMIT PostgreSQL
```

Required:

```text
commit PostgreSQL
    -> capture committed changes
    -> update projections asynchronously
```

Domain invariants MUST NOT depend on projections being current.

### 12.3 Router and acknowledgment

The CDC router converts source changes into domain-relevant jobs.

One change MAY create several jobs:

```text
source change
    -> search projection job
    -> notification job
    -> matching job
```

The router MUST:

* validate the change shape;
* derive stable job identifiers;
* enqueue all required jobs;
* apply bounded backpressure;
* acknowledge or reject the Sequin delivery.

The delivery is acknowledged only after all required jobs have been added to their bounded in-memory queues.

If any enqueue fails, the delivery MUST NOT be acknowledged.

A redelivery may therefore create duplicate jobs. All handlers MUST be idempotent.

### 12.4 MVP delivery guarantee

The MVP has no durable worker inbox, job queue, dead-letter table, or processed-job table.

After Sequin acknowledgment, jobs exist only in memory until processing completes.

If the worker process dies after acknowledgment, queued jobs may be lost.

The current guarantee is therefore:

```text
before acknowledgment:
    retryable delivery

after acknowledgment:
    best-effort in-memory processing
```

The system MUST NOT claim exactly-once or durable at-least-once processing.

This is an explicit MVP trade-off and MUST remain documented until durable worker delivery is introduced.

### 12.5 Idempotency and ordering

Handlers MUST tolerate:

* duplicate delivery;
* concurrent delivery;
* replay;
* an older change arriving after a newer one.

Use stable domain identifiers where possible:

```text
product jobs:
    product_events.event_id

shop jobs:
    (shop_id, version, operation)

search-filter jobs:
    (user_search_filter_id, version, operation)

match jobs:
    (
        user_search_filter_id,
        product_id,
        origin_event_id
    )
```

Projection records SHOULD store the latest applied source version.

An older or equal version MUST NOT overwrite a newer projection state.

Idempotency SHOULD be enforced in the target write through conditional updates, unique constraints, or version checks rather than through in-memory checks.

### 12.6 Building projections

A CDC payload may not contain enough information to build a complete projection.

In that case, treat the change as an invalidation signal:

```text
CDC change
    -> extract affected identifier
    -> read current committed PostgreSQL state
    -> build complete projection
    -> conditionally update target
```

Joined or hydrated projections SHOULD reread authoritative state rather than incrementally merging unrelated partial table changes.

Projection mapping belongs to the target adapter.

```text
authoritative query result
    -> adapter-local mapping
    -> search document / key-value item
```

Projection storage types MUST NOT escape their adapter.

### 12.7 Replay and rebuild

Every rebuildable projection MUST document:

* its authoritative source;
* its mapping;
* its source-version strategy;
* how it is rebuilt;
* how live changes are handled during rebuild;
* how the rebuilt projection is verified and activated.

Search indexes SHOULD use versioned indexes and an atomic alias or equivalent cutover.

Existing projections MUST NOT be treated as the recovery source for authoritative data.

### 12.8 Schema evolution

Database migrations affecting CDC consumers MUST use expand-and-contract changes where practical:

1. add new fields;
2. deploy producers;
3. deploy consumers;
4. rebuild or migrate projections;
5. remove old fields.

Tables or columns consumed by CDC MUST NOT be renamed or removed without reviewing:

* Sequin configuration;
* router logic;
* deserialization;
* projection handlers;
* replay and rebuild procedures.

Unknown additive fields SHOULD be tolerated. Missing required fields MUST fail explicitly.

### 12.9 Failure handling

Transient failures SHOULD be retried with bounded backoff.

Structurally invalid or permanently unprocessable changes MUST NOT be silently discarded.

Because the MVP has no durable dead-letter storage, poison changes require operator intervention, code or data correction, and replay.

Logs MUST contain safe identifiers and error categories, not complete source rows, credentials, tokens, or sensitive payloads.

### 12.10 Observability

Monitor at least:

* PostgreSQL replication-slot lag;
* retained WAL growth;
* Sequin delivery lag and retries;
* unacknowledged change age;
* router failures;
* queue depth and saturation;
* handler failures and latency;
* duplicate and stale-version rejections;
* projection freshness;
* projection rebuild status.

Structured logs SHOULD include:

```text
source
table or stream
operation
entity identifier
source version
job type
idempotency key
attempt
outcome
correlation identifier
```

### 12.11 Required tests

CDC tests SHOULD cover:

* insert, update, and delete mapping;
* duplicate delivery;
* concurrent delivery;
* stale changes;
* partial enqueue followed by redelivery;
* queue saturation;
* projection version checks;
* replay;
* full projection rebuild.

The accepted crash-after-ack loss window MUST remain covered by documentation or an operational test.

### 12.12 Non-negotiable rules

1. Every dataset has one operational owner.
2. Only committed PostgreSQL changes are propagated.
3. Projection stores are never part of PostgreSQL transactions.
4. Sequin is acknowledged only after all required jobs are enqueued.
5. All jobs and projection writes are idempotent.
6. Older source versions cannot overwrite newer projections.
7. Projections are rebuildable from authoritative truth.
8. Poison changes are never silently discarded.
9. CDC lag, queue pressure, failures, and freshness are observable.
10. The current post-ack in-memory loss risk is an explicit MVP limitation.


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

Use-case errors describe outcomes relevant to callers. Variants MUST name the concrete failure a caller can act on; avoid catch-all policy variants when the real cause is known, such as `AuthenticatedActorRequired`, `PlanDoesNotAllowAction`, or `ProductNotOwnedByActor`.

```text
NotFound
AuthenticatedActorRequired
Conflict
InvalidInput
TemporarilyUnavailable
Internal
```

They MAY wrap internal errors privately but MUST expose stable variants.

### Adapter errors

Adapter errors describe technical failures:

```text
Connection
Timeout
Decode
Mapping
ConcurrencyConflict
UnexpectedRowCount
ExternalResponse
```

They MUST NOT escape to controllers directly.

### HTTP mapping

Controllers map service errors to HTTP status and response DTOs.

The service MUST NOT return HTTP status codes.

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

### Log-Levels

Treat `error!` level as a burning fire and for bugs.
Use info and warn accordingly.
Use debug when sensible.
Less is more as long as everything important is covered.

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

### 15.2 Service-owned principal model

Controllers map transport principals into service-owned types:

```rust
pub enum Principal {
    Anonymous,
    User(UserId),
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

### 15.6 Operational identity logging

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

Secrets MUST NOT be embedded in domain/application types, logs, errors, or committed configuration files.

External clients and pools SHOULD be constructed once and shared through cloneable handles such as `PgPool` or `Arc<Client>`.

---

## 17. Concurrency and idempotency

### Optimistic concurrency

Aggregate tables SHOULD contain a monotonically increasing version.

Updates MUST include the loaded version:

```sql
UPDATE records
SET
    title = $1,
    version = version + 1
WHERE id = $2
  AND version = $3
RETURNING version
```

No returned row MUST map to an internal concurrency-conflict error when the row was expected to exist. Do not leak the concrete version value in errors. The returned version is authoritative only for PostgreSQL internals and CDC/outbox consumers; ordinary use cases SHOULD NOT return it.

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
- service-error-to-HTTP mapping.

A controller MUST NOT:

- access a database client;
- call a repository;
- build SQL or search DSL;
- compose multiple data sources;
- enforce domain invariants;
- mutate aggregates;
- return storage types;
- decide transaction scope.

Canonical flow:

```text
HTTP request
    -> REST request DTO
    -> service command/request
    -> use-case trait
    -> service result/view
    -> REST response DTO
    -> HTTP response
```

---

## 19. Async traits and dispatch

Inbound use-case traits are commonly stored as trait objects in use-case bundles:

```rust
Arc<dyn SearchRecordsUseCase>
```

Async trait methods used through `dyn Trait` MUST use the workspace's object-safe async-trait convention, currently `async_trait`.

```rust
#[async_trait::async_trait]
pub trait SearchRecordsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: SearchRecordsRequest,
    ) -> Result<SearchRecordsResult, SearchRecordsError>;
}
```

The attribute MUST also be applied to implementations.

Outbound ports MAY use static dispatch when practical. Consistency is preferred over mixing several async trait styles inside one domain crate.

---

## 20. Testing strategy

### Core tests

Core tests MUST verify domain behavior without infrastructure.

Test:

- valid transitions;
- rejected transitions;
- invariants;
- event emission, when the aggregate uses domain events;
- version changes;
- idempotent no-op behavior.

### Service tests

Datastore-independent handlers MUST be tested with fakes or mocks for their ports.

Test:

- orchestration;
- batching;
- fallback behavior;
- optional enrichment;
- preservation of search order;
- error translation;
- authorization decisions.

### PostgreSQL adapter tests

PostgreSQL repositories and readers SHOULD have integration tests against a real PostgreSQL instance.

Test:

- `FromRow` mappings;
- aggregate rehydration;
- insert/update semantics;
- optimistic concurrency;
- rollback behavior;
- cross-entity transactions;
- joined readers;
- migration compatibility.

### Other adapter tests

Each adapter SHOULD test:

- request serialization;
- response deserialization;
- mapping to application types;
- timeout/error mapping.

### Controller tests

Controller tests SHOULD verify:

- DTO deserialization;
- DTO/use-case mapping;
- status-code mapping;
- response serialization;
- authentication/context mapping;
- missing-token behavior on public routes;
- invalid-token rejection on public and protected routes;
- protected-route authentication enforcement.

They SHOULD mock only the inbound use-case trait, not repositories.

### Acceptance tests

Acceptance tests SHOULD:

- verify cross-layer correctness of the most important paths from the outside, working ONLY against the REST API
- be written as theses on the systems behavior

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
| `...Item` | Key-value representation |
| `...Record` | External/graph/source response representation |
| `...Dto` | Transport representation |
| `...Event` | Domain event |

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

Names SHOULD describe business intent or application capability.

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

## 23. Implementation-agent checklist

Agents MUST read this document before implementing architecture-affecting work.

### Before coding

- [ ] Identify the bounded context and aggregate.
- [ ] Identify whether the change is a command, query, projection, or infrastructure concern.
- [ ] Place use-case input/output types in one `service/use_cases/...` file.
- [ ] Define or reuse the smallest capability-oriented outbound ports.
- [ ] Confirm that no port is named after a database.
- [ ] Decide whether the final read model belongs to the use case.
- [ ] Decide whether the write requires one PostgreSQL transaction.
- [ ] Identify invariant-critical reads that must use the same transaction.
- [ ] Identify optional additional-source enrichment and its failure behavior.
- [ ] Define error translation at each layer.
- [ ] Define mapping location for every boundary type.

### While coding

- [ ] Keep domain fields private.
- [ ] Keep adapter rows/documents/items private.
- [ ] Use `FromRow` for PostgreSQL row deserialization.
- [ ] Use `TryFrom` where mapping can fail.
- [ ] Bind domain values to SQL inside the DAO/repository implementation.
- [ ] Avoid public domain-to-row conversions for writes.
- [ ] Batch hydration queries.
- [ ] Preserve source ordering after hydration.
- [ ] Use chained temporary repositories for one-off transactional calls.
- [ ] Explicitly commit successful SQLx transactions.
- [ ] Add a use-case tracing span with safe structured fields.
- [ ] Map transport authentication into service-owned `OperationContext`.
- [ ] Ensure protected mutations reject anonymous principals.
- [ ] Log relevant committed actions with actor, target, and outcome.
- [ ] Do not leak adapter errors or types across boundaries.
- [ ] Use the narrowest possible visibility.

### Before completion

- [ ] Add domain unit tests.
- [ ] Add use-case orchestration tests.
- [ ] Add adapter mapping/integration tests.
- [ ] Add transaction/concurrency tests where relevant.
- [ ] Add controller DTO/error mapping tests.
- [ ] Add acceptance tests
- [ ] Verify no N+1 access pattern was introduced.
- [ ] Verify no service/core import points toward an adapter.
- [ ] Verify no controller accesses a repository or database client.
- [ ] Verify logs contain no secrets or sensitive payloads.
- [ ] Update this document when a new general architectural rule was introduced.

---

## 24. Pull-request review checklist

Reviewers SHOULD reject changes that cannot answer these questions clearly:

1. Which layer owns each new type?
2. Is each public type intentionally public?
3. Does the use case express business intent?
4. Does the handler depend only on required capabilities?
5. Is aggregate persistence separated from read-model construction?
6. Are storage mappings confined to adapters?
7. Does the transaction contain exactly the invariant-critical PostgreSQL work?
8. Are cross-source reads composed in a handler rather than a controller?
9. Is trusted caller identity carried through `OperationContext` rather than request input?
10. Do relevant mutations record actor, target, and committed outcome safely?
11. Are errors and logs safe and appropriately translated?
12. Are the important rules covered by tests?

---

## 25. Informative references

These references explain library behavior used by the conventions above:

- SQLx `FromRow`: <https://docs.rs/sqlx/latest/sqlx/trait.FromRow.html>
- SQLx `Transaction`: <https://docs.rs/sqlx/latest/sqlx/struct.Transaction.html>
- SQLx `query_as`: <https://docs.rs/sqlx/latest/sqlx/fn.query_as.html>
- `async-trait`: <https://docs.rs/async-trait/latest/async_trait/>
- `tracing`: <https://docs.rs/tracing/latest/tracing/>

The rules in this document remain authoritative even when a referenced implementation library changes. Library upgrades MUST be reviewed for effects on these conventions.
