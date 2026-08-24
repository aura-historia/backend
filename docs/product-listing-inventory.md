# ProductListing rewrite inventory

## Baseline and method

- Checked-out commit: `5fff6ffd397f61bea047ac67c122c5437fad8cf2`.
- The runbook reviewed `d47b0245e58f7c151f705017b3c793cdb4793f91`; implementation must reconcile any baseline drift before a later iteration.
- Inventory searches were run on 2026-08-24. `rg` is unavailable in this environment, so repository content search used the equivalent expressions. Repeat the final negative scans with `rg` when available.

## Classification rules

| Class | Action |
| --- | --- |
| Aura-owned listing contract | Rename or delete. |
| Provider-native contract | May retain provider wording; map explicitly at the adapter boundary. |
| Human-facing copy | May say “product” when natural language requires it. |
| Unrelated bounded context/history | Keep only when unrelated or an intentional changelog/history reference. |

## Confirmed Aura-owned blast radius

### Workspace and crate family

- Root: `Cargo.toml`, `Cargo.lock`, `depgraph-rules.toml`, CI/package selectors, and workspace wiring reference the five `product-*` crates.
- Crates: `src/product-core`, `src/product-service`, `src/product-postgres`, `src/product-opensearch`, and `src/product-translation-llm`.
- Consumers include API/runtime/worker/test composition plus `search-filter-*`, `watchlist-*`, `notification-*`, crawler, Shopify, CI determination, and infrastructure.

### Core, service, persistence, and events

- `ProductState`, `ProductLifecycle`, and `ProductSaleValuation` are imported through API and core/service-facing contracts.
- Product events, event history payloads, sale valuation fields, and current-event revision semantics require one coordinated rename.
- `migrations/20260725090000_initial_business_schema.sql` contains `products`, `product_events`, `product_translations`, `product_watchlist`, `product_id`, `product_slug_id`, `shops_product_id`, state checks, search-filter match foreign keys, and notification references.
- Crawler initial schema and normalization mappings must be separately audited for six-value state mirrors and product identifiers.

### API and public contracts

- `src/aura-historia-api/src/lib.rs` registers public `/api/v1/products` and partner `/api/v1/shops/{shop_id}/products` routes.
- `src/aura-historia-api/src/products/**`, `partner_products/**`, `watchlist/**`, `search_filters/**`, `notifications/**`, `wire.rs`, `error.rs`, and `state.rs` carry Aura-owned product/state types, fields, errors, and test fixtures.
- Confirmed DTO and wire locations include `products/product_data.rs`, `products/product_event_data.rs`, `products/search_products.rs`, `partner_products/types.rs`, `notifications/types.rs`, and `wire.rs`.
- Public docs include `docs/swagger.yaml` and `docs/CHANGELOG.md`; historical changelog prose must be classified rather than mechanically rewritten.

### Search, projection, and downstream contexts

- OpenSearch mapping/index assets under `opensearch/mappings` and `src/product-opensearch` require rename and optional availability handling.
- CDC routing, source-version checks, worker jobs, percolation, saved filters, notification sources, watchlist readers, translations, embeddings, and acceptance fixtures use listing identifiers and must be audited together.
- Search-filter match records and notifications contain `product_id` and product-event relationships in the initial business schema.
- MJML/templates and `infra` may contain route, index, queue, or human-copy references; classify each hit before changing it.

### Source boundaries

- Crawler normalization, URL metadata, demo/review snapshots, and crawler mappings are Aura-owned ACL work.
- `src/shopify-lambda` provider DTOs and webhook shapes retain Shopify wording but must map into ProductListing contracts.
- WooCommerce ingestion in the renamed service crate follows provider-native wording at input and ProductListing commands beyond the boundary.
- schema.org vocabulary remains adapter-local and must not become a core mirror enum.

## Iteration checklist

- [ ] 1. Rename crate directories, package names, imports, dependency rules, composition wiring, and crate DOX without forwarding crates.
- [ ] 2. Rename Aura-owned Rust types, ports, readers, use cases, documents, and tests to `ProductListing` vocabulary without aliases.
- [ ] 3. Rename initial schemas, routes, DTO fields, errors, event/table/index/CDC identifiers, OpenSearch assets, docs, and fixtures.
- [ ] 4. Replace `ProductState` with optional `ListingAvailability` and `ListingLifecycle::{Active, Withdrawn}`; make core events deterministic payloads.
- [ ] 5. Separate `ListingSaleObservation`, including its dedicated transactional use case and presentation FX policy.
- [ ] 6. Install crawler, Shopify, WooCommerce, and schema.org ACLs with non-destructive uncertainty handling.
- [ ] 7. Add null-aware availability/orderability search and final REST patch semantics.
- [ ] 8. Rebuild projection and downstream consumers around active-listing membership.
- [ ] 9. Remove legacy vocabulary, refresh docs/DOX, reset development data, and run final negative scans and full gates.

## Required final scans

Run and classify all remaining results before completion:

```sh
rg -n --hidden --glob '!target/**' 'product-core|product-service|product-postgres|product-opensearch|product-translation-llm'
rg -n --hidden --glob '!target/**' 'ProductState|ProductLifecycle|ProductSaleValuation|DeleteProduct|mark_removed|mark_unknown|mark_sold'
rg -n --hidden --glob '!target/**' 'PRODUCT_STATE_CHANGED|DOMAIN_STATE_CHANGED|PRODUCT_DELETED|\bLISTED\b|\bUNKNOWN\b|\bREMOVED\b'
rg -n --hidden --glob '!target/**' 'products|product_id|product_slug_id|shops_product_id|product_events|product_translations|product_watchlist'
rg -n --hidden --glob '!target/**' '/products|productId|productSlugId|shopsProductId|stateQuery'
```

Permitted residual `product` terminology is limited to provider-native contracts, natural-language copy, unrelated intrinsic-product contexts, and intentional issue/history references.
