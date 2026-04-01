# Database

All state is stored in PostgreSQL. The schema lives in `src/crawler/sql/schema.sql`.

---

## Tables

### `shops`

The top-level entity. One row per shop.

| Column | Type | Notes |
|--------|------|-------|
| `shop_id` | UUID PK | |
| `url_pattern` | TEXT (nullable) | LLM-discovered regex that matches product page URLs. `NULL` until the spider classifies the shop for the first time. |
| `created` | TIMESTAMPTZ | |
| `updated` | TIMESTAMPTZ | |

`url_pattern` is the handoff from the URL classification LLM to the spider's per-URL classification logic. Once set, it is reused on subsequent crawls and only refreshed if it matches zero products.

---

### `shop_domains`

A shop may be reachable via multiple domains. Each domain gets its own row so crawl scheduling and locking are per-domain.

| Column | Type | Notes |
|--------|------|-------|
| `domain_id` | UUID PK | Auto-generated (`gen_random_uuid()`) |
| `shop_id` | UUID FK → `shops` | Cascade on delete |
| `shop_domain` | TEXT UNIQUE | e.g. `antiques-shop.de` or `https://antiques-shop.de` |
| `last_crawled` | TIMESTAMPTZ (nullable) | Set at the end of each successful crawl |
| `locked_at` | TIMESTAMPTZ (nullable) | Optimistic distributed lock — set to `NOW()` when a worker starts, cleared on finish |

**Lock semantics**: a worker acquires the lock with a conditional `UPDATE ... WHERE locked_at IS NULL OR locked_at < NOW() - INTERVAL '30 minutes'`. This makes the lock self-healing: a crashed worker's lock expires automatically after 30 minutes.

---

### `shop_urls`

Every URL the spider has ever seen. Shared between the spider (writes) and the scraper (reads + updates).

| Column | Type | Notes |
|--------|------|-------|
| `url` | TEXT PK | Normalised URL (no fragment, no trailing slash) |
| `shop_id` | UUID FK → `shops` | Cascade on delete |
| `url_class` | TEXT | One of `product`, `category`, `imprint`, `info`, `other` |
| `main_hash` | TEXT (64 chars) | SHA-256 of the page HTML, updated by the spider on each crawl |
| `state` | TEXT | `UNKNOWN` \| `LISTED` \| `AVAILABLE` \| `RESERVED` \| `SOLD` \| `REMOVED` |
| `price_currency` | TEXT (nullable) | ISO 4217 code, populated by scraper |
| `price_value` | INT (nullable) | Amount in minor units (cents), populated by scraper |
| `last_scraped_hash` | TEXT (nullable) | `main_hash` value at the time of the last successful scrape |
| `last_scraped` | TIMESTAMPTZ (nullable) | Timestamp of the last successful scrape |
| `created` / `updated` | TIMESTAMPTZ | |

**Change detection**: the scraper compares `main_hash` (current) with `last_scraped_hash` (last seen). If they match the page has not changed and the fetch is skipped.

**Index**: `idx_shop_urls_class_last_scraped ON shop_urls (url_class, last_scraped)` supports the scraper candidate query efficiently.

---

### `shops_product_schema`

Caches the LLM-generated CSS selector schema for each shop. One row per shop.

| Column | Type | Notes |
|--------|------|-------|
| `shop_id` | UUID PK | |
| `product_schema` | JSONB | Serialized `ProductCssSelectorSchema` |
| `created` / `updated` | TIMESTAMPTZ | |

If a schema fails to apply to a product page, the LLM is asked to fix it and the repaired schema overwrites this row.

---

### `product_state_mapping`

Translation table from raw state strings scraped from pages (e.g. `"Nur noch 2 verfügbar"`) to normalised `UrlState` values. Shared across all shops.

| Column | Type | Notes |
|--------|------|-------|
| `raw` | TEXT PK | Trimmed, lower-cased input — or a regex pattern string |
| `normalized` | TEXT | One of `LISTED`, `AVAILABLE`, `RESERVED`, `SOLD`, `REMOVED`, `UNKNOWN` |
| `mapping_type` | TEXT | `VALUE` (exact match) or `REGEX` (pattern match) |
| `created` / `updated` | TIMESTAMPTZ | |

The schema seeds ~50 common exact-value mappings (EN/DE/FR/ES/IT) and ~25 regex patterns for quantity-style strings. Novel strings fall through to the LLM and are then persisted here so future lookups are instant.

**Index**: `idx_product_state_mapping_regex ON product_state_mapping (mapping_type) WHERE mapping_type = 'REGEX'` — partial index so the regex scan (`find_all_regex_mappings`) only reads regex rows.

---

## Key Query Patterns

### Batch upsert into `shop_urls` (UNNEST)

The spider writes up to 100 rows at a time using PostgreSQL `UNNEST` to avoid N individual statements:

```sql
INSERT INTO shop_urls (url, shop_id, url_class, main_hash, state, created, updated)
SELECT * FROM UNNEST($2::text[], $3::text[], $4::text[], $5::text[])
       AS t(url, url_class, main_hash)
...
ON CONFLICT (url) DO UPDATE SET
    url_class  = EXCLUDED.url_class,
    main_hash  = EXCLUDED.main_hash,
    updated    = NOW()
```

### Optimistic lock acquire

```sql
UPDATE shop_domains
SET    locked_at = NOW()
WHERE  shop_domain = $1
  AND  (locked_at IS NULL OR locked_at < NOW() - INTERVAL '30 minutes')
RETURNING domain_id
```

Returns a row only if the lock was successfully acquired; 0 rows = already locked.

### Spider candidate selection

```sql
SELECT s.shop_id, sd.shop_domain
FROM   shops s
JOIN   shop_domains sd ON sd.shop_id = s.shop_id
WHERE  sd.last_crawled IS NULL
   OR  sd.last_crawled < NOW() - INTERVAL '7 days'
LIMIT  $1
```

### Scraper candidate selection

```sql
SELECT shop_id, url, main_hash, last_scraped_hash
FROM   shop_urls
WHERE  url_class = 'product'
  AND  state IN ('UNKNOWN', 'LISTED', 'AVAILABLE', 'RESERVED')
  AND  (last_scraped IS NULL OR last_scraped < NOW() - INTERVAL '1 day')
LIMIT  $1
```
