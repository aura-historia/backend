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

## Iteration 3 — money and localization

| Current path/type | Kind | Canonical consumers | Legacy consumers | Features | Semantic owner | Target | Action | Compatibility shim | Deletion prerequisite |
|---|---|---|---|---|---|---|---|---|---|
| `common::currency::domain::{Currency, MinorUnitExponent, HasMinorUnitExponent}` | domain value and minor-unit rules | Product, Shop, User, Search Filter, Watchlist, FX, API, Worker, and canonical runtime fixtures | Existing legacy crates | `test-data` | reusable money domain | `money` | split | none; legacy and canonical currency types remain separate to avoid legacy fixed-FX behavior crossing the boundary | Legacy consumers migrate or retire |
| `common::price::domain::{MonetaryAmount, Price}` | domain value | Product, Search Filter, Watchlist, FX, API, Worker, and canonical adapters | Existing legacy crates, including fixed-rate behavior | `test-data` | reusable money domain | `money` | split | none; `FixedFxRate` and its historical `Price` helpers remain legacy-only | Legacy fixed-FX path has no consumers |
| `common::language::domain::Language` | domain value | Product, Shop, User, Search Filter, Notification, API, Worker, and canonical runtime fixtures | Existing legacy crates | `test-data` | reusable localization domain | `localization` | split | none; storage/transport conversions are private at their owning boundaries | Legacy consumers migrate or retire |
| `common::localized::Localized<L, T>` | generic domain wrapper | Product, Search Filter, API, Worker, and canonical adapters | Existing legacy crates | `test-data` | reusable localization domain | `localization` | split | none; legacy `TextRecord`/`LocalizedTextData` conversions stay outside the leaf crate | Legacy consumers migrate or retire |
| `common::measurement_unit::*` | user preference plus DTO/record mappings | User core, User Postgres, API, and canonical adapter fixtures | Existing legacy crates | `test-data` | User domain | `user-core` | split | none; canonical mapping is private in User Postgres/API | Legacy consumers migrate or retire |

## Iteration 4 — shop and geo ownership

| Current path/type | Kind | Canonical consumers | Legacy consumers | Features | Semantic owner | Target | Action | Compatibility shim | Deletion prerequisite |
|---|---|---|---|---|---|---|---|---|---|
| `common::{shop_id::ShopId, shop_name::ShopName, shop_slug_id::ShopSlugId, seller_slug_id::SellerSlugId, domain::{Domain, NoDomainError}}` | shop identifiers and host value | Shop, Product, Partner Shop, Notification, API, PostgreSQL, and runtime crates | Existing legacy crates | `test-data` | Shop domain | `shop-core` | move | `common` type aliases; legacy slug conversions remain in shim | Legacy Shop consumers migrate |
| `common::distance::domain::{Distance, DistanceUnit, GeoDistanceQuery}` | geo domain values | Product, Search Filter, API, PostgreSQL, and OpenSearch crates | Existing legacy crates | `test-data` | Geo domain | `geo::core` | move | `common::distance::domain` aliases; legacy DTO conversions stay in `common::distance::data` | Legacy geo consumers migrate |
| `Distance::opensearch_value` | OpenSearch formatting | `product-opensearch` | Legacy Product and User OpenSearch adapters | none | OpenSearch boundary | `geo::opensearch` | split | none | Legacy adapters use `geo::opensearch` directly |
