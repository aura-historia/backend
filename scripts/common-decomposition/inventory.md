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

## Iteration 5 — FX-rate ownership

| Current path/type | Kind | Canonical consumers | Legacy consumers | Features | Semantic owner | Target | Action | Compatibility shim | Deletion prerequisite |
|---|---|---|---|---|---|---|---|---|---|
| `common::fx_rate_id::FxRateId` | FX snapshot identifier | FX, Product, Search Filter, Watchlist, API, Worker, and PostgreSQL/OpenSearch adapters | Legacy Product and legacy runtimes | `test-data` | FX-rate domain | `fxrate-core` | move | `common::fx_rate_id` re-export | Legacy consumers migrate to `fxrate-core` |
| `common::price::domain::{FixedFxRate, FxRate, FX_RATE_SCALE}` | historical fixed-rate conversion | none | Legacy Product, API, and acceptance paths | `test-data` | legacy compatibility | `common` | retain-legacy | none | Proven legacy consumers migrate or retire |

## Iteration 6 — Product ownership

| Current path/type | Kind | Canonical consumers | Legacy consumers | Features | Semantic owner | Target | Action | Compatibility shim | Deletion prerequisite |
|---|---|---|---|---|---|---|---|---|---|
| `common::product_id::{ProductId, ProductKey}` | Product identity and canonical shop-product key | Product core/service/adapters, API, worker, Search Filter, Watchlist, Notification | Legacy Product and runtimes | `test-data` | Product domain | `product-core` | split | `common::product_id` re-export; legacy API/key helpers remain local | Legacy Product consumers migrate; legacy boundary helpers retire |
| `common::{product_slug_id::ProductSlugId, shops_product_id::ShopsProductId}` | Product slug and shop-local Product ID | Product core/service/adapters, API, worker, Search Filter, Watchlist, Notification | Legacy Product and runtimes | `test-data` | Product domain | `product-core` | move | `common` re-exports with legacy `SlugId` conversions | Legacy consumers migrate |
| `common::product_state::domain::ProductState` | Product state | Product core/service/adapters and canonical consumers | Legacy Product state API/DynamoDB paths | `test-data` | Product domain | `product-core` | split | No direct re-export: legacy state retains legacy localization/boundary behavior | Legacy Product state boundary mappings migrate |
| `common::product_lifecycle::domain::ProductLifecycle` | Product lifecycle | Product core/service/adapters and Search Filter | Legacy Product lifecycle record/document paths | `test-data` | Product domain | `product-core` | split | No direct re-export: legacy record/document conversions remain local | Legacy Product lifecycle boundary mappings migrate |
| `common::query::{AnyOfQuery, RangeQuery, TextQuery}` | Domain-neutral query values | Product core and canonical search consumers | Existing legacy crates | `test-data` | domain-neutral primitives | `domain-primitives` | move | `common::query` re-exports | Legacy consumers migrate |
| `product-core::user_state::*` | Product user-state presentation values | Product, Search Filter, Watchlist, API, Postgres readers | none | none | Product application read model | `product-service` | move | none | Consumers use `product-service::user_state` |

## Iteration 8 — search filter, watchlist, partner shop, notification

| Current path/type | Kind | Canonical consumers | Legacy consumers | Features | Semantic owner | Target | Action | Compatibility shim | Deletion prerequisite |
|---|---|---|---|---|---|---|---|---|---|
| `common::{user_search_filter_id::UserSearchFilterId, user_search_filter_name::UserSearchFilterName, enhanced_match_reason::EnhancedMatchReason}` | search-filter ID and values | Search Filter, Notification | Existing legacy search-filter paths | `test-data` | Search Filter domain | `search-filter-core` | move | `common` re-export | Legacy callers migrate to `search-filter-core` |
| `common::resource_state::domain::ResourceState` | generic legacy lifecycle | Search Filter and Watchlist had independent policies | Existing legacy resource users | `test-data` | Separate Search Filter and Watchlist domains | `search-filter-core::SearchFilterState`, `watchlist-core::WatchlistState` | split | none; old state remains legacy | Legacy users migrate or retire |
| `common::partner_shop_application_id::PartnerShopApplicationId` | partner application ID | Partner Shop, Notification | Existing legacy partner-shop paths | `test-data` | Partner Shop Application domain | `shop-partner-core` | move | `common` re-export | Legacy callers migrate to `shop-partner-core` |
| `common` notification identifiers | cross-domain notification payload references | Notification core | Existing legacy notification paths | `test-data` | Referenced entity cores | `user-core`, `search-filter-core`, `shop-partner-core`, `domain-primitives` | split | not applicable | Legacy notification path migrates |
| `common::resource_state::document::ResourceStateDocument` | legacy OpenSearch document form | Search Filter OpenSearch adapter | Existing legacy OpenSearch paths | `opensearch` | Search Filter OpenSearch adapter | `search-filter-opensearch` local mapping | split | none | Legacy document form retires |

## Iteration 9 — runtime transport cutover

| Current path/type | Kind | Canonical consumers | Legacy consumers | Features | Semantic owner | Target | Action | Compatibility shim | Deletion prerequisite |
|---|---|---|---|---|---|---|---|---|---|
| `common::{error::boxed, pagination::cursor::{Cursor, CursoredResult}, patch_field::PatchField, personalized::Personalized}` | technology-neutral application contracts | API, worker, canonical services and adapters | Existing legacy crates | `test-data` for pagination fakes | shared application layer | `application` | move | `common` re-exports the core contracts; legacy API shapes remain local | Legacy users migrate from `common` |
| `common::sort::{Sort, SortOrder}` | domain-neutral query values | API and canonical read/query contracts | Existing legacy crates | `api` for legacy extraction | domain primitives | `domain-primitives` | move | `common::sort` re-exports values; legacy API extractor remains local | Legacy users migrate from `common` |
| `common::personalized::api::PersonalizedData`, `common::pagination::cursor::api::JsonCursoredData`, `common::resource_state::data::*` | REST DTOs | `aura-historia-api` | Existing legacy API crates | `api` | Axum transport | `aura-historia-api` | split | none | Legacy API consumers retire or migrate |
| `common` direct dependency in API and worker | legacy compatibility dependency | `aura-historia-api`, `aura-historia-worker` | none | `api` in API only | runtime/transport plus existing owners | API and worker crates | move | none | Removed in this iteration |

## Iteration 7 — credential and `OperationContext` ownership

| Current path/type | Kind | Canonical consumers | Legacy consumers | Features | Semantic owner | Target | Action | Compatibility shim | Deletion prerequisite |
|---|---|---|---|---|---|---|---|---|---|
| `common::operation_context::{OperationContext, Principal, CredentialCapability}` | caller identity and authorization context | Canonical services and API mapping | Legacy APIs and services | none | `application` plus `credential-core` scope vocabulary | `application` / `credential-core` | move/split | `common::operation_context` re-export | Legacy callers migrate; transport forms stay at the edge |
| `common::oauth_client_id::OAuthClientId` | credential identifier | OAuth service and adapters | Legacy OAuth paths | none | credential identifiers | `credential-core` | move | `common::oauth_client_id` re-export | Legacy OAuth callers migrate |

The credential vocabulary now has no cycle through `common`: `application` owns the operation contract, while `credential-core` owns scope and credential identifiers. Legacy shims remain for old callers.

## Shared OpenSearch protocol boundary

`platform-opensearch` owns the proven generic `search_response` wire envelope used by
`product-opensearch` and `search-filter-opensearch`. It contains no bounded-context
search document: each adapter keeps its document type private and supplies it as `T`.
`common::opensearch::search_response` is an acyclic legacy re-export. No other
OpenSearch client, response, query, or document helper moved without separate sharing proof.

## Current status — follow-up Iterations 4 and 5 complete

The ten canonical service crates, seven canonical PostgreSQL adapters, and listed canonical
integrations no longer have a normal or development `common` dependency or import
`common::*`. The cutover covers `billing-stripe`, `fxrate-fxratesapi`,
`large-language-model`, `notification-dynamodb`, `oauth-dynamodb`, `product-opensearch`,
`product-postgres`, `search-filter-opensearch`, `search-filter-postgres`,
`shop-partner-postgres`, `shop-postgres`, `user-dynamodb`, `user-postgres`, `user-zoho`,
`watchlist-postgres`, and `fxrate-postgres`.

PostgreSQL adapters own SQL rows, parameters, and mappings. OpenSearch adapters own documents,
queries, and mappings; `platform-opensearch` owns only the generic search-response envelope.
DynamoDB adapters own update, batch, and record mechanics.
`large-language-model` owns LLM vocabulary and invocation metrics/logging; the
observability platform owns subscriber setup only. Legacy `common` APIs, entities, APIs,
Lambdas, and tests remain.

The current machine baseline has 44 normal direct consumers, 12 development edges, 10
forwarded-package feature records (30 forwarded feature entries), seven declared `common`
features, and 56 public top-level modules. Six canonical runtime/leaf consumers remain:
`cloudwatch-log-retention-lambda`, `cognito`, `cognito-post-confirmation`, `fxrate-lambda`,
`shopify-lambda`, and `stripe-lambda`.

Validation for this slice:

- Targeted Iteration 4 and 5 package checks passed with `--all-targets --all-features`.
- All listed adapter tests passed, including configured Postgres, DynamoDB, and OpenSearch tests.
- `cargo depgraph-check check`, `check_baseline.py`, and baseline unit tests passed.
- `cargo check --workspace`, CI Clippy, and `cargo fmt --all -- --check` passed.
