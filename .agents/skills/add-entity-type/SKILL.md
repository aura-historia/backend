---
name: add-entity-type
description: Use when adding or changing a domain entity, value object, ID, enum, command, DTO, DynamoDB Record, or OpenSearch Document in the Aura Historia Rust backend. Covers naming, newtypes, domain behavior, conversions, DynamoDB key shape, OpenSearch mappings, docs, and tests.
---

# Add Entity Type

Use this skill when a task adds or changes backend types across domain, REST, DynamoDB, OpenSearch, or service command layers.

## First checks

- Read required `AGENTS.md`: repo root, `src/AGENTS.md`, target crate `AGENTS.md`, and docs area when touched.
- Inspect the same crate first. Good samples:
  - common enum all layers: `src/common/src/language/`
  - string newtype: `src/product/src/core/title.rs`, `src/common/src/domain.rs`
  - ID/key newtypes: `src/common/src/product_id.rs`, `src/common/src/shop_id.rs`
  - commands: `src/shop/src/service/command.rs`, `src/product/src/service/product_command.rs`
  - records: `src/shop/src/dynamodb/shop_record.rs`, `src/product/src/dynamodb/product_record.rs`
  - documents: `src/product/src/opensearch/product_document.rs`, `src/shop/src/opensearch/shop_document.rs`
  - mappings: `opensearch/mappings/*.json`
- Decide what kind of type it is before coding:
  - entity/aggregate root
  - value object
  - identifier/key/slug
  - enum/state
  - command
  - REST DTO
  - DynamoDB persistence record
  - OpenSearch document/read model

## Layer names

Keep suffixes strict:

- Domain: no suffix, e.g. `Language`, `Shop`, `Product`, `Title`
- REST API DTO: `Data`, e.g. `LanguageData`, `GetShopData`
- DynamoDB: `Record`, e.g. `ShopRecord`, `LanguageRecord`
- OpenSearch: `Document`, e.g. `ShopDocument`, `LanguageDocument`
- Service intent: `Command`, e.g. `CreateShopCommand`, `UpdateProductCommand`
- Update payload/read helper may use local existing names, e.g. `ShopRecordUpdate`, `ProductDocumentUpdate`

Do not pass `Data`, `Record`, or `Document` types into domain logic unless the existing crate pattern already does conversion there. Domain methods should speak domain types.

## Module home

- Domain/value object: `src/<crate>/src/core/<name>.rs`
- REST DTO: `src/<crate>/src/data/<name>_data.rs`
- DynamoDB record: `src/<crate>/src/dynamodb/<name>_record.rs`
- OpenSearch document: `src/<crate>/src/opensearch/<name>_document.rs`
- Commands/services: `src/<crate>/src/service/command*.rs`
- Common reusable primitive: `src/common/src/<name>.rs` or a focused submodule like `language/`
- Export through each module `mod.rs`; expose crate modules behind existing feature gates in `lib.rs`.

Do not create new module docs. Create/update crate root `AGENTS.md` only when shape, purpose, env, dependency edge, test flow, or child index changes.

## Domain design

- Put invariant and behavior in domain types. No anemic model.
- Prefer newtypes over naked `String`, `Uuid`, or primitive IDs when the value has meaning.
- Keep services thin. They orchestrate repositories/adapters and call domain methods.
- Make illegal state hard to build:
  - use constructors or `TryFrom` for validation
  - use private fields when callers must not bypass checks
  - return typed errors with `thiserror`
- Use domain methods for transitions, e.g. `change_state`, `delete`, `personalize`.
- If a state change emits an event, create it in the domain method, not in API/Lambda glue.
- Prefer `Option<Event>` for idempotent no-op transitions.
- Avoid `unwrap`, `expect`, `panic`, `.ok()` in production paths.

## Newtype choices

Use existing helpers where they fit:

- `common::uuid_v4_newtype!(TypeId)` for random entity IDs.
- `common::uuid_v7_newtype!(EventIdLike)` for time-sortable event IDs when needed.
- `common::slug_id_newtype!(TypeSlugId, suffix_len)` for URL/search slugs.
- `common::string_newtype!(Name)` for plain semantic strings.
- `common::string_newtype!(Name, max_length(N))` for trimmed bounded strings.
- `common::string_newtype!(Name, struct_only)` when custom normalization/validation is needed.
- Hand-write the type when parsing can fail, e.g. domain extraction from URLs.

Rules:

- `From` only for infallible/lossless conversions or intentional normalization.
- `TryFrom` for validation that can fail.
- Implement `Display`, `AsRef<str>`, `From<Type> for String`, or `Deref<Target = str>` only when useful and consistent.
- Add `#[cfg_attr(feature = "test-data", derive(fake::Dummy))]` or custom `Dummy<Faker>` when tests need generated data.

## Commands

- Commands are service intent, not REST payloads.
- `Create*Command` should contain required domain fields.
- `Update*Command` usually uses `Option<T>` for patchable fields and may implement `Default`.
- Add `is_empty()` for patch commands when empty updates are invalid.
- Implement `HasKey` when commands are batched/deduped by aggregate key.
- Implement `Mergeable` when queue/event workers merge repeated updates for the same key.
- Map `Data -> Command` in API/Lambda edge or nearby adapter code. Do not put HTTP details in services.

## REST Data DTOs

- Use `Serialize`/`Deserialize` and `#[serde(rename_all = "camelCase")]` for object payloads.
- Use explicit enum serde shape (`lowercase`, `SCREAMING_SNAKE_CASE`, etc.) to match public contract.
- Keep request DTOs separate from response DTOs when fields differ.
- Add extractor helpers under `#[cfg(feature = "api")]` when path/query parsing repeats.
- Map body/query/path errors to `ApiError` with correct field detail.
- Update `docs/swagger.yaml` and `docs/CHANGELOG.md` when public payload changes.

## DynamoDB Records

- Record structs own persistence shape only.
- Use `#[derive(Serialize, Deserialize)]` and field names that match table attributes.
- Add `#[serde(skip_serializing_if = "Option::is_none", default)]` for sparse optional fields.
- Use `time::serde::rfc3339` helpers for timestamps.
- Flatten nested fields when table/index/query shape needs it, e.g. `structured_address_*`, `geo_address_*`.
- Add key helpers next to the record:
  - `mk_pk(...)`
  - `mk_sk(...)`
  - `mk_gsi*_pk(...)`
  - `mk_gsi*_sk(...)`
- Keep key prefixes stable and explicit, e.g. `shop#shop_id#{shop_id}`, `product#materialized`.
- Implement `HasKey` for records that are batch processed or cross-store synced.
- Use `TryFrom<Record> for Domain` when required fields may be missing or invalid.
- Use `From<Domain> for Record` only when the mapping cannot fail.
- Update repository queries/writes and integration tests when keys or indexes change.
- Update `docs/dynamodb/table_1.md` when key shape, item shape, GSI, TTL, or event-relevant attribute changes.

## OpenSearch Documents

- Document structs own search/read-model shape only.
- Use `#[serde(rename_all = "camelCase")]` for document field names unless existing index says otherwise.
- Add `_id()` when repository indexes by document ID.
- Implement `HasKey` when sync logic needs aggregate keys.
- Use `From<Record> for Document` or `TryFrom<Record/Event> for Document` based on fallibility.
- Keep search denormalization intentional. Do not copy DynamoDB shape blindly.
- If a DTO/document field changes, update corresponding mapping in `opensearch/mappings/*.json`.
- Check analyzers, keyword/text choice, numeric/date/geo types, nested/object shape, and vector dimensions.
- Update OpenSearch repository tests and sync Lambda flow when document mapping changes.

## Conversions

- Prefer explicit `impl From<X> for Y` / `impl TryFrom<X> for Y`; avoid ad-hoc mapping in many call sites.
- Keep conversions near the type that owns the target shape, following local pattern.
- Fallible persistence mappings should return typed mapping errors, e.g. `PersistenceMappingError` + `MissingPersistenceField` where used.
- Avoid lossy conversions unless the type name or method makes it clear.
- Test round trips where meaningful:
  - domain ↔ data
  - domain ↔ record
  - record → document
  - key string ↔ key type

## Feature gates and dependencies

- Keep `core` always available.
- Gate optional layers through existing crate features:
  - `data`
  - `dynamodb`
  - `opensearch`
  - `service`
  - `test-data`
- Add dependencies to the crate `Cargo.toml` only when needed by that feature/layer.
- Prefer workspace dependencies.
- Do not create cross-crate dependency cycles. Move shared primitives to `common` when multiple domains need them.

## Tests

Cover type behavior, not only serialization:

- domain invariants and state transitions
- newtype normalization and failure cases
- command `is_empty`, `HasKey`, and `Mergeable`
- serde representation for `Data`, `Record`, and `Document`
- `From`/`TryFrom` conversion success and failure
- DynamoDB key helpers and repository integration tests
- OpenSearch document mapping/repository integration tests
- fake `Dummy<Faker>` generation when feature `test-data` is supported

Use `rstest` for table tests. Use `fake` where crate supports it. Test names should say what happens: `should_*_when_*`.

## Docs and DOX

Update in same change when shape or contract changes:

- nearest crate `AGENTS.md`: purpose, modules, data/event/env/test shape
- `src/AGENTS.md` child index only for new top-level crates
- `docs/swagger.yaml` and `docs/CHANGELOG.md` for REST contract changes
- `docs/dynamodb/table_1.md` for table shape/key/index changes
- `docs/events/flow.md` for event shape/flow changes
- `opensearch/mappings/*.json` for document/index changes

## Validation

Start narrow:

1. `cargo check -p <crate>`
2. `cargo test -p <crate> --all-features`
3. If shared common/domain type changed, run affected API/Lambda crate checks.
4. If DynamoDB/OpenSearch repo changed, run targeted integration tests.
5. If OpenSearch mappings changed, run affected sync/search tests.
6. If docs/infra/event flow changed, run related validation from those areas.
7. For broad changes: `cargo check --workspace`.
