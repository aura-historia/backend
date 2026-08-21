# `common` decomposition inventory

Decomposition baseline and iteration record. Legacy `common` paths remain only as documented compatibility shims.

Machine guard data: `scripts/common-decomposition/baseline.json`.
Run it with:

```sh
python3 scripts/common-decomposition/check_baseline.py
```

The machine baseline must match Cargo exactly and may only shrink. CI pins the committed
baseline for bootstrap, then compares later baselines with their base revision; a
consumer, feature, forwarded feature, or public module cannot be added by changing the
baseline.

Iterations 3–5 removed `common` from the ten canonical services, seven canonical
PostgreSQL adapters, and the remaining canonical integrations. The current baseline has
44 normal direct consumers, 12 development edges, 10 forwarded feature records (30
forwarded entries), 7 declared `common` features, and 56 public top-level modules.
Only six canonical runtime/leaf consumers remain; all listed canonical adapters are
now `common`-free. Legacy entity/API/Lambda paths remain unchanged.

## Direct consumers

Class meanings:

- **canonical** — a canonical adapter, runtime, or canonical leaf still using legacy `common`.
- **legacy** — an old entity/API/Lambda path retained for compatibility.
- **dual** — composition or test code spanning both paths.

Current machine-checked normal direct consumers:

| Consumer | Class | Normal features |
|---|---|---|
| `acceptance-tests` | dual | `opensearch` |
| `aura-historia-parent` | dual | — |
| `aws-tests-common` | legacy | `opensearch` |
| `cloudwatch-log-retention-lambda` | canonical | — |
| `cognito` | canonical | `api` |
| `cognito-post-confirmation` | canonical | `postgres` |
| `crawler` | legacy | — |
| `fxrate-lambda` | canonical | `postgres` |
| `newsletter-api` | legacy | `api` |
| `notification` | legacy | — |
| `notification-api` | legacy | `api` |
| `notification-send` | legacy | `dynamodb`, `event_bridge` |
| `oauth` | legacy | `api` |
| `oauth-api` | legacy | `api` |
| `partner-shop-application` | legacy | — |
| `partner-shop-application-api` | legacy | `api` |
| `partner-shop-application-lambda` | legacy | — |
| `product` | legacy | — |
| `product-api` | legacy | `api`, `opensearch` |
| `product-api-partner` | legacy | `api`, `opensearch` |
| `product-lambda-delete-product` | legacy | `dynamodb`, `event_bridge`, `opensearch` |
| `product-lambda-ingest-partner-products` | legacy | `api`, `opensearch`, `sqs` |
| `product-lambda-materialize-opensearch` | legacy | `api`, `dynamodb`, `event_bridge`, `opensearch` |
| `product-lambda-update-notify-user` | legacy | `dynamodb`, `event_bridge` |
| `product-personalization` | legacy | `api` |

| `product-watchlist` | legacy | — |
| `product-watchlist-api` | legacy | `api`, `opensearch` |
| `search-filter` | legacy | — |
| `search-filter-api` | legacy | `api`, `opensearch` |
| `search-filter-lambda-opensearch-sync` | legacy | `api`, `dynamodb`, `event_bridge`, `opensearch` |
| `search-filter-lambda-percolate-product` | legacy | `api`, `dynamodb`, `event_bridge`, `opensearch` |
| `search-filter-periodic-match` | legacy | `api`, `dynamodb`, `opensearch` |

| `shop` | legacy | — |
| `shop-api` | legacy | `api`, `opensearch` |
| `shop-lambda-opensearch-index` | legacy | `api`, `dynamodb`, `event_bridge`, `opensearch` |
| `shopify-lambda` | canonical | `postgres` |
| `stripe-api` | legacy | `api` |
| `stripe-lambda` | canonical | `postgres` |
| `test-api` | dual | — |
| `user` | legacy | — |
| `user-api` | legacy | `api`, `opensearch` |
| `user-lambda-index-opensearch` | legacy | `api`, `dynamodb`, `event_bridge`, `opensearch` |
| `user-lambda-tier-update` | legacy | `dynamodb`, `event_bridge` |
| `webhook-api` | legacy | `api`, `opensearch` |

Current direct-consumer counts: **6 canonical**, **35 legacy**, **3 dual**.

The development-only direct edges are listed in `scripts/common-decomposition/baseline.json` and are also exact-checked by CI.

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
| `change_outcome` | generic outcome | services | legacy services | — | domain primitives | `domain-primitives` | move | yes if acyclic | canonical imports moved |
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
| `logging` | logging + LLM vocabulary | legacy runtimes | Lambdas | — | runtime setup; LLM adapter | split | legacy init only | roots and LLM moved |
| `measurement_unit` | preference value + forms | user | legacy user | — | user or neutral preference | needs-owner-decision | needs-owner-decision | no | owner decided |
| `mergeable` | capability trait | cores | legacy | — | proven neutral trait | `domain-primitives` | needs-owner-decision | no | usage review |
| `oauth_client_id` | typed ID | OAuth | old OAuth | — | credential vocabulary | `credential-core` | move | yes | OAuth canonical moved |
| `opensearch` | client/responses/doc helpers | OpenSearch adapters | old APIs/Lambdas | `opensearch` | adapter boundary | consuming adapters | split | none | canonical adapters moved |
| `operation_context` | auth context/capabilities | services/API | legacy | — | application | `application` | move | yes | credential cycle resolved |
| `pagination` | query wrappers + API forms | services | APIs | — | application; API owns forms | `application` | split | domain wrapper only | API forms local |
| `partner_shop_application_id` | typed ID | partner core | old partner | — | partner shop | `shop-partner-core` | move | yes | canonical imports moved |
| `patch_field` | generic patch field | services | APIs | — | application | `application` | move | yes | canonical imports moved |
| `personalized` | generic personalized view + DTO | services | APIs | — | application; API owns DTO | split | wrapper only | DTO local |
| `postgres` | SQLx config/UoW | adapters/runtimes | Lambdas | `postgres` | platform | `platform-postgres` | move | legacy config only | canonical imports moved |
| `price` | money/FX values + forms | FX/product | APIs | — | money; FX behavior stays FX | `money` | split | domain only | forms local |
| `product_id` | ID/key + API forms | product core/service | product APIs | — | product | `product-core` | split | ID only | API forms local |
| `product_lifecycle` | product lifecycle + forms | product core | legacy product | — | product | `product-core` | split | domain only | forms local |
| `product_slug_id` | product slug ID | product core | product APIs | — | product | `product-core` | split | ID only | API extraction local |
| `product_state` | product state | product core | legacy product | — | product | `product-core` | move | yes | canonical imports moved |
| `query` | generic query values | services/readers | APIs | — | domain primitives | `domain-primitives` | move | yes | canonical imports moved |
| `resource_state` | resource state + forms | product | legacy product | — | product if product-only | needs-owner-decision | needs-owner-decision | no | ownership review |
| `seller_slug_id` | seller slug ID | shop/product | legacy | — | shop or seller context | needs-owner-decision | needs-owner-decision | no | ownership review |
| `serde` | date serialization | adapters/API | legacy | — | owning boundary | boundary local | split | no | mapping local |
| `shop_id` | shop ID + API extraction | shop/product | shop APIs | — | shop | `shop-core` | split | ID only | extraction local |
| `shop_name` | shop name | shop core | partner APIs | — | shop | `shop-core` | move | yes | canonical imports moved |
| `shop_slug_id` | shop slug ID | shop/product | legacy | — | shop | `shop-core` | move | yes | canonical imports moved |
| `shops_product_id` | external-shop product ID | product core | product APIs | — | product | `product-core` | split | ID only | extraction local |
| `slug_id` | generic slug machinery | cores | legacy | — | neutral primitive | `domain-primitives` | move | yes | canonical imports moved |
| `sort` | generic sort + API parsing | services | APIs | `api` for legacy extraction | domain-primitives; API owns parsing | split | domain only | parsing local |
| `string_newtype` | newtype macro | cores | legacy | — | neutral primitive | `domain-primitives` | move | yes | canonical imports moved |
| `stripe_customer_id` | payment customer ID | billing/user | Stripe | — | user core | `user-core` | move | no | Legacy Stripe callers migrate |
| `transaction` | transaction contracts | services/adapters | legacy | — | application | `application` | move | yes | canonical imports moved |
| `user_id` | user ID + API extraction | all services | user APIs | — | user | `user-core` | split | ID only | extraction local |
| `user_search_filter_id` | typed ID + API extraction | filter core | filter APIs | — | search filter | `search-filter-core` | split | ID only | extraction local |
| `user_search_filter_name` | filter name | filter core | legacy filter | — | search filter | `search-filter-core` | move | yes | canonical imports moved |
| `utm` | URL tracking helper | APIs/crawler | legacy | — | owning web boundary | needs-owner-decision | needs-owner-decision | no | usage review |
| `uuid_newtype` | newtype macros | cores | legacy | — | neutral primitive | `domain-primitives` | move | yes | canonical imports moved |
| `version` | version macro/error | repositories | legacy | — | neutral primitive | `domain-primitives` | move | yes | canonical imports moved |
| `versioned` | generic version wrapper | repositories | legacy | — | domain primitives | `domain-primitives` | move | yes | canonical imports moved |
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

## Follow-up Iteration 4 — canonical PostgreSQL adapter cutover

The seven canonical PostgreSQL adapters now use `platform-postgres` for SQLx transaction
mechanics and direct service/core owners for IDs, values, query contracts, and errors:
`fxrate-postgres`, `product-postgres`, `search-filter-postgres`, `shop-partner-postgres`,
`shop-postgres`, `user-postgres`, and `watchlist-postgres`. Rows, SQL parameters, persisted
strings, and mapping helpers remain private to each adapter. Their `common` dependencies
and baseline entries are gone.

## Follow-up Iteration 5 — canonical integration cutover

The canonical Stripe, FxRatesApi, OpenSearch, DynamoDB, Zoho, OAuth, and notification
adapters now own their provider/storage mechanics locally. OpenSearch documents remain private;
`platform-opensearch` owns the proven generic search-response envelope. DynamoDB update/batch
helpers do not escape their adapters. `notification-dynamodb`
is canonical-only and `common`-free; no legacy notification dependency was removed.

LLM-specific operation, provider/model, service-tier, metrics, and invocation logging
vocabulary now lives in `large-language-model`; `platform-observability` remains subscriber
setup only. Crawler callers use the LLM crate directly. The remaining direct `common`
consumers are legacy paths, three dual composition/test roots, and six canonical runtime
leaves tracked in the table above.
