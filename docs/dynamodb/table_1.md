# DynamoDB Partition Layout

Single-table design. Table name pattern: `table_1-{stage}` (e.g. `table_1-dev`).

Migration note: business truth is moving to Postgres. This table remains current for not-yet-migrated entities and the long-term DynamoDB owners documented in `docs/storage.md`.

Stream: `NEW_IMAGE`, feeds the `DynamoDbEventBus` via an EventBridge Pipe.

TTL attribute: `ttl` (Unix timestamp).

---

## Entities & Key Patterns

### Product — Event Records

Each product change is stored as an immutable event. The sort key encodes the event category and a UUIDv7 `event_id` (lexicographically sortable by creation time).

| Attribute | Pattern |
|-----------|---------|
| `pk` | `product#shop_id#{shop_id}#shops_product_id#{shops_product_id}` |
| `sk` | `product#event#domain#{event_id}` |
| `sk` | `product#event#enrichment#{event_id}` |
| `sk` | `product#event#policy#{event_id}` |
| `sk` | `product#event#lifecycle#{event_id}` |

**Event types stored in `event_type`:**

| Category | `event_type` value |
|----------|-------------------|
| Domain | `DOMAIN_CREATED` |
| Domain | `DOMAIN_STATE_CHANGED` |
| Domain | `DOMAIN_PRICE_CHANGED` |
| Domain | `DOMAIN_ESTIMATE_PRICE_CHANGED` |
| Domain | `DOMAIN_URL_CHANGED` |
| Domain | `DOMAIN_IMAGES_CHANGED` |
| Domain | `DOMAIN_AUCTION_TIME_CHANGED` |
| Enrichment | `ENRICHMENT_TRANSLATED_TITLE` |
| Enrichment | `ENRICHMENT_EMBEDDED` |
| Policy | `POLICY_PROHIBITED_CONTENT_DECISION` |
| Lifecycle | `LIFECYCLE_DELETED` |

---

### Product — Materialized View

One record per product, updated on every relevant event. Queried by `ShopId + ShopsProductId` or resolved from slug IDs via GSI2. Soft deletion sets `lifecycle = DELETED`.

| Attribute | Pattern |
|-----------|---------|
| `pk` | `product#shop_id#{shop_id}#shops_product_id#{shops_product_id}` |
| `sk` | `product#materialized` |
| `gsi2_pk` | `shop_slug_id#{shop_slug_id}#product_slug_id#{product_slug_id}` |
| `gsi2_sk` | `product#lookup#shop_id#shops_product_id` |

---

### Shop

| Attribute | Pattern |
|-----------|---------|
| `pk` | `shop#shop_id#{shop_id}` |
| `sk` | `shop#details` |
| `gsi1_pk` | `partner_user#{partner_user_id}` _(sparse – only for partner shops)_ |
| `gsi1_sk` | `partner_shop_id#{shop_id}` _(sparse – only for partner shops)_ |
| `gsi2_pk` | `shop_slug_id#{shop_slug_id}` _(sparse)_ |
| `gsi2_sk` | `shop#lookup#shop_id` _(sparse)_ |
| `gsi3_pk` | `shop#shopify_domain#{shopify_domain}` _(sparse – only for Shopify partner shops)_ |
| `gsi3_sk` | `shop#details` _(sparse – only for Shopify partner shops)_ |

---

### Partner Shop Application

| Attribute | Pattern |
|-----------|---------|
| `pk` | `user_id#{user_id}` |
| `sk` | `partner_shop_application_id#{partner_shop_application_id}` |
| `gsi1_pk` | `global#partner_shop_application` |
| `gsi1_sk` | `partner_shop_application_id#{partner_shop_application_id}` |

---

### User

| Attribute | Pattern |
|-----------|---------|
| `pk` | `user#{user_id}` |
| `sk` | `user#details` |

---

### Watchlist Product

One record per (user, product). LSI1 allows sorting by creation time; GSI1 allows querying all watchers of a given product.

| Attribute | Pattern |
|-----------|---------|
| `pk` | `user#{user_id}` |
| `sk` | `product#watch#shop_id#{shop_id}#shops_product_id#{shops_product_id}` |
| `lsi1_sk` | `product#watch#created#{nanoseconds_20_digits}` |
| `gsi1_pk` | `product_id#{product_id}` |
| `gsi1_sk` | `watch#user#{user_id}` |

---

### User Search Filter

| Attribute | Pattern |
|-----------|---------|
| `pk` | `user#{user_id}` |
| `sk` | `search_filter#{search_filter_id}` |

---

### Search Filter Match

Records which products matched a user's saved search filter. LSI1 allows paginating matches sorted by creation time. LSI2 allows querying one user's matches for a specific product. GSI2 allows product-delete cleanup by `product_id`.

| Attribute | Pattern |
|-----------|---------|
| `pk` | `user#{user_id}` |
| `sk` | `search_filter_match#search_filter#{search_filter_id}#shop_id#{shop_id}#shops_product_id#{shops_product_id}` |
| `lsi1_sk` | `search_filter_match#{nanoseconds_20_digits}` |
| `lsi2_sk` | `search_filter_match#shop_id#{shop_id}#shops_product_id#{shops_product_id}#{nanoseconds_20_digits}` _(sparse)_ |
| `gsi2_pk` | `product_id#{product_id}` |
| `gsi2_sk` | `search_filter_match#user_id#{user_id}` |

**Bounds for LSI1 range queries:**

| Bound | Value |
|-------|-------|
| Lower | `search_filter_match#` |
| Upper | `search_filter_match#\u{ffff}` |

---

### Notification

One record per (user, origin event). LSI1 routes to either watchlist or search-filter notification lists; LSI2 allows looking up all notifications for a specific product.

| Attribute | Pattern |
|-----------|---------|
| `pk` | `user#{user_id}` |
| `sk` | `user#notification#origin_event_id#{origin_event_id}` |
| `lsi1_sk` | `user#notification#watchlist#{notification_id}` |
| `lsi1_sk` | `user#notification#search_filter#{notification_id}` |
| `lsi2_sk` | `user#notification#product_id#{product_id}#origin_event_id#{origin_event_id}` _(sparse)_ |
| `ttl` | Unix timestamp (7-day expiry) |

**Bounds for LSI2 range queries:**

| Bound | Value |
|-------|-------|
| Prefix | `user#notification#product_id#` |

---


## Indexes Summary

### Local Secondary Indexes

| Index | PK | SK | Used by |
|-------|----|----|---------|
| `lsi1` | same as table PK | `lsi1_sk` | Watchlist (sort by created), Search Filter Match (sort by created), Notification (route by reason) |
| `lsi2` | same as table PK | `lsi2_sk` | Notification (query by product), Search Filter Match (query by product) |

### Global Secondary Indexes

| Index | PK | SK | Projection | Used by |
|-------|----|----|------------|---------|
| `gsi1` | `gsi1_pk` | `gsi1_sk` | All attributes | Watchlist — query all watchers of a `product_id`; Shop — query partner shops by `partner_user_id`; Partner Shop Application — query all applications |
| `gsi2` | `gsi2_pk` | `gsi2_sk` | Keys only | Product — slug → `(shop_id, shops_product_id)` lookup; Shop — slug → `shop_id` lookup; Search Filter Match — product-delete cleanup by `product_id` |
| `gsi3` | `gsi3_pk` | `gsi3_sk` | All attributes | Shop — Shopify domain → shop lookup |
