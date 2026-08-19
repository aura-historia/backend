# Common Decomposition Inventory

## Iteration 1 — transaction foundation

| Current path/type | Kind | Canonical consumers | Legacy consumers | Features | Semantic owner | Target | Action | Compatibility shim | Deletion prerequisite |
|---|---|---|---|---|---|---|---|---|---|
| `common::transaction::{Transaction, UnitOfWork, TransactionError}` | application contract | Product, search-filter, shop, shop-partner, user, and watchlist services | Existing legacy crates | none | shared application layer | `application` | move | `common::transaction` re-export | Legacy consumers migrate to `application` |
| `common::postgres::{SqlxUnitOfWork, SqlxTransaction}` | SQLx mechanics | Canonical PostgreSQL adapters; API and worker composition | Existing legacy runtimes | `postgres` | shared PostgreSQL platform | `platform-postgres` | move | `common::postgres` re-export | Legacy consumers migrate to `platform-postgres` |
| `common::postgres::{PostgresPoolConfig, connect_from_env}` | runtime configuration | API and worker now parse `POSTGRES_*` locally | Existing legacy runtimes | `postgres` | runtime composition roots plus PostgreSQL platform | API/worker + `platform-postgres` | split | Legacy `common::postgres` env shim | Legacy runtimes parse environment at their composition roots |

## Iteration 2 — domain primitives and observability

| Current path/type | Kind | Canonical consumers | Legacy consumers | Features | Semantic owner | Target | Action | Compatibility shim | Deletion prerequisite |
|---|---|---|---|---|---|---|---|---|---|
| `common::change_outcome::ChangeOutcome` | domain-neutral outcome | `product-core`, `shop-core`, `user-core`, `search-filter-core` | Existing legacy crates | none | domain primitives | `domain-primitives` | move | `common::change_outcome` re-export | Legacy consumers migrate to `domain-primitives` |
| `common::{event::Event, event_id::EventId}` | generic event envelope and ID | `product-core`, canonical services/adapters pending later slices | Existing legacy crates and EventId API extraction | `api`, `test-data` | domain primitives | `domain-primitives` | split | `common::event`/`event_id` re-export; EventId API extraction remains legacy-local | Legacy consumers migrate; legacy API extraction is retired |
| `common::{version, versioned, uuid_newtype, string_newtype}` | generic value/newtype machinery | Available to canonical owners; migration begins in later owner slices | Existing legacy crates | `test-data` | domain primitives | `domain-primitives` | move | Legacy `common` copies remain because macro re-export would expand its guarded public surface | Legacy consumers migrate to `domain-primitives` |
| `common::logging::{init_logging, init_logging_with_directives}` | subscriber setup | `aura-historia-api`, `aura-historia-worker` | Existing legacy runtimes | none | observability platform | `platform-observability` | split | `common::logging` delegates setup | Legacy runtime migration completes |
| `common::batch::Batch` | mixed bounded collection and AWS helpers | No proven second canonical semantic use | Existing legacy crates | `dynamodb`, `sqs`, `test-data` | needs owner decision | `common` | retain-legacy | none | Split pure collection from AWS mappings after usage review |
