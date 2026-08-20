# `common` decomposition inventory

Decomposition baseline and iteration record. Legacy `common` paths remain only as documented compatibility shims.

Machine guard data: `scripts/common-decomposition/baseline.json`.
Run it with:

```sh
python3 scripts/common-decomposition/check_baseline.py
```

The baseline lists 81 normal direct consumers, 12 development edges, and all declared
forwarding to `common/*` features. It must match Cargo exactly and may only shrink. CI
pins this initial baseline for bootstrap, then compares later baselines with their base
revision; a consumer, feature, forwarded feature, or public module cannot be added by
changing the baseline.

## Direct consumers

Class meanings:

- **canonical** — a canonical core/service/adapter/runtime or its canonical leaf.
- **legacy** — an old entity/API/Lambda path retained for compatibility.
- **dual** — composition or test code spanning both paths.

| Consumer | Class | Normal features | Development features |
|---|---|---|---|
| `acceptance-tests` | dual | `opensearch` | `test-data` |
| `aura-historia-api` | canonical | `api`, `postgres` | — |
| `aura-historia-parent` | dual | — | — |
| `aura-historia-worker` | canonical | `postgres` | — |
| `aws-tests-common` | legacy | `opensearch` | — |
| `billing-service` | canonical | — | — |
| `billing-stripe` | canonical | — | — |
| `cloudwatch-log-retention-lambda` | canonical | — | — |
| `cognito` | canonical | `api` | — |
| `cognito-post-confirmation` | canonical | `postgres` | — |
| `crawler` | legacy | — | — |
| `fxrate-core` | canonical | — | — |
| `fxrate-fxratesapi` | canonical | — | — |
| `fxrate-lambda` | canonical | `postgres` | — |
| `fxrate-postgres` | canonical | `postgres` | — |
| `fxrate-service` | canonical | — | — |
| `large-language-model` | canonical | — | — |
| `newsletter-api` | legacy | `api` | `test-data` |
| `notification` | legacy | — | — |
| `notification-api` | legacy | `api` | — |
| `notification-core` | canonical | — | — |
| `notification-dynamodb` | dual | `dynamodb` | — |
| `notification-send` | legacy | `dynamodb`, `event_bridge` | — |
| `notification-service` | canonical | — | — |
| `oauth` | legacy | `api` | — |
| `oauth-api` | legacy | `api` | — |
| `oauth-core` | canonical | — | — |
| `oauth-dynamodb` | canonical | `dynamodb` | — |
| `oauth-service` | canonical | — | — |
| `partner-shop-application` | legacy | — | — |
| `partner-shop-application-api` | legacy | `api` | `test-data` |
| `partner-shop-application-lambda` | legacy | — | `test-data` |
| `product` | legacy | — | — |
| `product-api` | legacy | `api`, `opensearch` | — |
| `product-api-partner` | legacy | `api`, `opensearch` | `test-data` |
| `product-core` | canonical | — | — |
| `product-lambda-delete-product` | legacy | `dynamodb`, `event_bridge`, `opensearch` | — |
| `product-lambda-ingest-partner-products` | legacy | `api`, `opensearch`, `sqs` | `test-data` |
| `product-lambda-materialize-opensearch` | legacy | `api`, `dynamodb`, `event_bridge`, `opensearch` | — |
| `product-lambda-update-notify-user` | legacy | `dynamodb`, `event_bridge` | — |
| `product-opensearch` | canonical | `opensearch` | — |
| `product-personalization` | legacy | `api` | — |
| `product-postgres` | canonical | `postgres` | — |
| `product-service` | canonical | — | — |
| `product-watchlist` | legacy | — | — |
| `product-watchlist-api` | legacy | `api`, `opensearch` | — |
| `search-filter` | legacy | — | — |
| `search-filter-api` | legacy | `api`, `opensearch` | — |
| `search-filter-core` | canonical | — | — |
| `search-filter-lambda-opensearch-sync` | legacy | `api`, `dynamodb`, `event_bridge`, `opensearch` | — |
| `search-filter-lambda-percolate-product` | legacy | `api`, `dynamodb`, `event_bridge`, `opensearch` | `test-data` |
| `search-filter-opensearch` | canonical | `opensearch` | — |
| `search-filter-periodic-match` | legacy | `api`, `dynamodb`, `opensearch` | `test-data` |
| `search-filter-postgres` | canonical | `postgres` | — |
| `search-filter-service` | canonical | — | — |
| `shop` | legacy | — | — |
| `shop-api` | legacy | `api`, `opensearch` | — |

| `shop-lambda-opensearch-index` | legacy | `api`, `dynamodb`, `event_bridge`, `opensearch` | — |
| `shop-partner-core` | canonical | — | — |
| `shop-partner-postgres` | canonical | `postgres` | — |
| `shop-partner-service` | canonical | — | — |
| `shop-postgres` | canonical | `postgres` | — |
| `shop-service` | canonical | — | — |
| `shopify-lambda` | canonical | `postgres` | `postgres` |
| `stripe-api` | legacy | `api` | `test-data` |
| `stripe-lambda` | canonical | `postgres` | — |
| `test-api` | dual | — | — |
| `user` | legacy | — | — |
| `user-api` | legacy | `api`, `opensearch` | `test-data` |
| `user-core` | canonical | — | — |
| `user-dynamodb` | canonical | `dynamodb` | — |
| `user-lambda-index-opensearch` | legacy | `api`, `dynamodb`, `event_bridge`, `opensearch` | — |
| `user-lambda-tier-update` | legacy | `dynamodb`, `event_bridge` | — |
| `user-postgres` | canonical | `postgres` | — |
| `user-service` | canonical | — | — |
| `user-zoho` | canonical | — | — |
| `watchlist-core` | canonical | — | — |
| `watchlist-postgres` | canonical | `postgres` | — |
| `watchlist-service` | canonical | — | — |
| `webhook-api` | legacy | `api`, `opensearch` | `test-data` |

Canonical direct-consumer baseline: **41**. Legacy: **35**. Dual: **4**.

## Export inventory

`common` has 56 public top-level modules. Direct dependency alone does not prove a
module use. Each move must run targeted source and feature searches before changing a
module. “Shim” means a temporary `common` re-export only after the new owner has no
`common` dependency.

| Current path/type | Kind | Canonical consumers | Legacy consumers | Features | Semantic owner | Target | Action | Compatibility shim | Deletion prerequisite |
|---|---|---|---|---|---|---|---|---|---|
| `actor` | principal + boundary forms | services | APIs/tests | — | application; adapters own forms | split | split | no storage shim | mappings local |
| `api` | API Gateway transport | — | old APIs/Lambdas | `api` | legacy edge | legacy `common` | retain-legacy | none | legacy edge gone |
| `batch` | batch helper | worker/adapters | Lambdas | — | proven neutral helper | `domain-primitives` or owner | needs-owner-decision | no | usage proves owner |
| `change_outcome` | generic outcome | services | legacy services | — | application primitive | `domain-primitives` | move | yes if acyclic | canonical imports moved |
| `currency` | money value + forms | cores/services | APIs | — | money | `money` | split | domain only | boundary forms local |
| `distance` | geo value + forms | search adapters | APIs | — | geo | `geo` | split | domain only | DTO/document mappings local |
| `domain` | host/domain value | shop/product | legacy shop/product | — | shop unless proven neutral | `shop-core` | needs-owner-decision | yes | owner decided |
| `dynamodb_stream` | stream/SQS extraction | worker | Lambdas | `event_bridge` | worker/runtime | worker or narrow AWS crate | split | none | canonical consumers moved |
| `dynamodb_update` | DynamoDB expression | Dynamo adapters | legacy Dynamo | `dynamodb` | Dynamo adapter | consuming adapter | split | none | canonical adapters moved |
| `enhanced_match_reason` | match value | filter core | notification/filter | — | search filter | `search-filter-core` | move | yes | canonical imports moved |
| `error` | error helpers + mapping errors | services/adapters | all | — | application; adapters own mapping errors | split | helper only | adapter errors local |
| `event` | generic event envelope | cores | legacy | — | neutral primitive | `domain-primitives` | move | yes | canonical imports moved |
| `event_id` | event ID + API extraction | worker | APIs | — | event owner; API owns extraction | split | ID only | extractors local |
| `execution_state` | job state + forms | worker | tests | — | worker/projection | worker | split | domain only | forms local |
| `fake` | test fixture | tests | tests | `test-data` | test support | `aws-tests-common` or local | move | no | test consumers moved |
| `fx_rate_id` | typed ID | FX crates | product/search | — | FX | `fxrate-core` | move | yes | FX canonical moved |
| `has_key` | capability trait | cores | legacy | — | proven neutral trait | `domain-primitives` | needs-owner-decision | no | usage review |
| `language` | localization value + forms | cores/adapters | APIs | — | localization | `localization` | split | domain only | forms local |
| `localized` | generic localization | cores | legacy | — | localization | `localization` | move | yes | canonical imports moved |
| `logging` | logging + LLM vocabulary | runtimes/LLM | Lambdas | — | runtime and LLM adapter | split | legacy init only | roots and LLM moved |
| `measurement_unit` | preference value + forms | user | legacy user | — | user or neutral preference | needs-owner-decision | needs-owner-decision | no | owner decided |
| `mergeable` | capability trait | cores | legacy | — | proven neutral trait | `domain-primitives` | needs-owner-decision | no | usage review |
| `oauth_client_id` | typed ID | OAuth | old OAuth | — | OAuth | `oauth-core` | move | yes | OAuth canonical moved |
| `opensearch` | client/responses/doc helpers | OpenSearch adapters | old APIs/Lambdas | `opensearch` | adapter boundary | consuming adapters | split | none | canonical adapters moved |
| `operation_context` | auth context/capabilities | services/API | legacy | — | application | `application` | move | yes | credential cycle resolved |
| `pagination` | query wrappers + API forms | services | APIs | — | application; API owns forms | split | domain only | API forms local |
| `partner_shop_application_id` | typed ID | partner core | old partner | — | partner shop | `shop-partner-core` | move | yes | canonical imports moved |
| `patch_field` | generic patch field | services | APIs | — | application | `application` | move | yes | canonical imports moved |
| `personalized` | generic personalized view + DTO | services | APIs | — | application; API owns DTO | split | wrapper only | DTO local |
| `postgres` | SQLx config/UoW | adapters/runtimes | Lambdas | `postgres` | platform | `platform-postgres` | move | legacy config only | canonical imports moved |
| `price` | money/FX values + forms | FX/product | APIs | — | money; FX behavior stays FX | `money` | split | domain only | forms local |
| `product_id` | ID/key + API forms | product core/service | product APIs | — | product | `product-core` | split | ID only | API forms local |
| `product_lifecycle` | product lifecycle + forms | product core | legacy product | — | product | `product-core` | split | domain only | forms local |
| `product_slug_id` | product slug ID | product core | product APIs | — | product | `product-core` | split | ID only | API extraction local |
| `product_state` | product state | product core | legacy product | — | product | `product-core` | move | yes | canonical imports moved |
| `query` | generic query values | services/readers | APIs | — | application | `application` | move | yes | canonical imports moved |
| `resource_state` | resource state + forms | product | legacy product | — | product if product-only | needs-owner-decision | needs-owner-decision | no | ownership review |
| `seller_slug_id` | seller slug ID | shop/product | legacy | — | shop or seller context | needs-owner-decision | needs-owner-decision | no | ownership review |
| `serde` | date serialization | adapters/API | legacy | — | owning boundary | boundary local | split | no | mapping local |
| `shop_id` | shop ID + API extraction | shop/product | shop APIs | — | shop | `shop-core` | split | ID only | extraction local |
| `shop_name` | shop name | shop core | partner APIs | — | shop | `shop-core` | move | yes | canonical imports moved |
| `shop_slug_id` | shop slug ID | shop/product | legacy | — | shop | `shop-core` | move | yes | canonical imports moved |
| `shops_product_id` | external-shop product ID | product core | product APIs | — | product | `product-core` | split | ID only | extraction local |
| `slug_id` | generic slug machinery | cores | legacy | — | neutral primitive | `domain-primitives` | move | yes | canonical imports moved |
| `sort` | generic sort + API parsing | services | APIs | — | application; API owns parsing | split | domain only | parsing local |
| `string_newtype` | newtype macro | cores | legacy | — | neutral primitive | `domain-primitives` | move | yes | canonical imports moved |
| `stripe_customer_id` | payment customer ID | billing/user | Stripe | — | user or billing | needs-owner-decision | needs-owner-decision | no | owner decided |
| `transaction` | transaction contracts | services/adapters | legacy | — | application | `application` | move | yes | canonical imports moved |
| `user_id` | user ID + API extraction | all services | user APIs | — | user | `user-core` | split | ID only | extraction local |
| `user_search_filter_id` | typed ID + API extraction | filter core | filter APIs | — | search filter | `search-filter-core` | split | ID only | extraction local |
| `user_search_filter_name` | filter name | filter core | legacy filter | — | search filter | `search-filter-core` | move | yes | canonical imports moved |
| `utm` | URL tracking helper | APIs/crawler | legacy | — | owning web boundary | needs-owner-decision | needs-owner-decision | no | usage review |
| `uuid_newtype` | newtype macros | cores | legacy | — | neutral primitive | `domain-primitives` | move | yes | canonical imports moved |
| `version` | version macro/error | repositories | legacy | — | neutral primitive | `domain-primitives` | move | yes | canonical imports moved |
| `versioned` | generic version wrapper | repositories | legacy | — | application/domain primitive | `domain-primitives` | move | yes | canonical imports moved |
| `year` | year value object | product/search | legacy | — | neutral value | `domain-primitives` | needs-owner-decision | no | usage review |

Root macro exports follow their defining module: `uuid_v4_newtype!`,
`uuid_v7_newtype!`, `string_newtype!`, `slug_id_newtype!`, and `version_newtype!`.

## Iteration order

1. Create neutral owners only where real canonical use proves sharing:
   `domain-primitives`, `money`, `localization`, and `application`.
2. Move entity IDs and pure values into their existing core crates. Add only acyclic
   legacy re-exports.
3. Move transaction mechanics and typed SQLx pool construction to `platform-postgres`.
   Keep environment parsing in composition roots.
4. Move adapter representations and mappings into their PostgreSQL, OpenSearch,
   DynamoDB, API, worker, and Lambda boundaries.
5. Move logging vocabulary to its product/provider owner. Re-run the inventory after
   each slice; shrink the baseline in the same change.

No compatibility shim exists in Iteration 0. A later shim must name its semantic owner
and deletion prerequisite in code or that iteration report.

## Iteration 4 — shop and geo ownership

`shop-core` now owns `Domain`, `ShopId`, `ShopName`, `ShopSlugId`, and `SellerSlugId`.
`geo::core` owns `Distance`, `DistanceUnit`, and `GeoDistanceQuery`; `geo::opensearch`
owns OpenSearch distance formatting. Canonical API, service, PostgreSQL, and OpenSearch
boundaries map their own transport and stored shapes.

`common` retains documented acyclic aliases for the moved Shop and geo domain paths. Delete
them only after legacy consumers migrate. `shop-core` is removed from the direct-consumer
baseline.

## Iteration 8 — search filter, watchlist, partner shop, notification

`search-filter-core` owns `UserSearchFilterId`, `UserSearchFilterName`,
`EnhancedMatchReason`, and `SearchFilterState`. `watchlist-core` owns a separate
`WatchlistState`; both preserve the existing three stored state values through their adapters.
`shop-partner-core` owns `PartnerShopApplicationId`. `notification-core` now directly uses
public identifiers from the semantic owner cores.

`common` re-exports the moved IDs, name, and match reason for legacy code. Each shim is
acyclic because its new owner does not depend on `common`; remove it only after legacy callers
have migrated. The four canonical core crates are removed from the direct-consumer baseline.
