# Common Decomposition Inventory

## Iteration 1 — transaction foundation

| Current path/type | Kind | Canonical consumers | Legacy consumers | Features | Semantic owner | Target | Action | Compatibility shim | Deletion prerequisite |
|---|---|---|---|---|---|---|---|---|---|
| `common::transaction::{Transaction, UnitOfWork, TransactionError}` | application contract | Product, search-filter, shop, shop-partner, user, and watchlist services | Existing legacy crates | none | shared application layer | `application` | move | `common::transaction` re-export | Legacy consumers migrate to `application` |
| `common::postgres::{SqlxUnitOfWork, SqlxTransaction}` | SQLx mechanics | Canonical PostgreSQL adapters; API and worker composition | Existing legacy runtimes | `postgres` | shared PostgreSQL platform | `platform-postgres` | move | `common::postgres` re-export | Legacy consumers migrate to `platform-postgres` |
| `common::postgres::{PostgresPoolConfig, connect_from_env}` | runtime configuration | API and worker now parse `POSTGRES_*` locally | Existing legacy runtimes | `postgres` | runtime composition roots plus PostgreSQL platform | API/worker + `platform-postgres` | split | Legacy `common::postgres` env shim | Legacy runtimes parse environment at their composition roots |
