# Database

All state is stored in PostgreSQL. The authoritative schema is defined by the versioned migrations
in `src/crawler/migrations/`.

---

## Local Development

A `docker-compose.yml` lives inside the `src/crawler/` directory. Manage it with the
PowerShell helpers in `src/crawler/scripts/` (run them from any directory — they self-locate):

| Script | What it does |
|--------|-------------|
| `.\scripts\db-down.ps1` | Stop the container (data volume is preserved) |
| `.\scripts\db-reset.ps1` | Destroy the volume and start fresh — replaces the old manual workflow |
| `.\scripts\db-status.ps1` | Show applied / pending migrations (`cargo sqlx migrate info`) |

`db-status.ps1` requires `sqlx-cli`:

```powershell
cargo install sqlx-cli --no-default-features --features rustls,postgres
```

Running the demo (no manual DB setup required — the binary handles everything):

```powershell
$env:GEMINI_API_KEY = "..."
cargo run -p crawler --bin demo
```

---

## Adding a New Migration

1. Create a new file in `src/crawler/migrations/` with the naming pattern
   `YYYYMMDDHHMMSS_description.sql` (e.g. `20260201120000_add_shop_currency.sql`).
2. Write the migration SQL. Use `IF NOT EXISTS` / `IF EXISTS` guards for safety.
3. Run `.\db-migrate.ps1` locally to apply it.
4. Deploy the new server binary to production — `sqlx::migrate!()` applies it automatically on startup.

---

## Production

The production `server` binary calls `sqlx::migrate!("migrations/")` immediately after connecting
to Postgres (before any other work). Deploying a new binary is the only step required to update
the production schema — no manual SQL execution needed.

---

## Tables

### `shops`

The top-level entity. One row per shop.

| Column | Type | Notes |
|--------|------|-------|
| `shop_id` | UUID PK | Sourced from the upstream shop service |
| `shop_name` | TEXT (nullable) | Human-readable display name, synced from the upstream shop service |
| `shop_slug` | TEXT (nullable) | URL-friendly slug identifier, synced from the upstream shop service |
| `active` | BOOLEAN NOT NULL DEFAULT TRUE | Soft-delete flag managed by shop sync. `TRUE` shops are crawl/scrape eligible; `FALSE` shops are ignored by candidate selection. |
| `url_pattern` | TEXT (nullable) | LLM-discovered regex that matches product page URLs. `NULL` until the spider classifies the shop for the first time. |
| `created` | TIMESTAMPTZ | |
| `updated` | TIMESTAMPTZ | Set to `NOW()` on every shop registration sync |

`shop_name` and `shop_slug` are populated (and kept up-to-date) by the shop registration sync loop. They are nullable because a shop row may also be created directly by the spider before a sync has run.

`active` enables soft-deactivation when a shop no longer exists upstream. Deactivated shops are retained for history but excluded from future spider/scraper candidate queries.

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
| `domain_id` | UUID FK → `shop_domains` | Cascade on delete — links the URL to the specific domain it was discovered from |
| `url_class` | TEXT | One of `product`, `category`, `imprint`, `info`, `other` |
| `state` | TEXT | `UNKNOWN` \| `LISTED` \| `AVAILABLE` \| `RESERVED` \| `SOLD` \| `REMOVED` |
| `last_scraped_hash` | TEXT (nullable) | Scraper-computed hash at the time of the last successful scrape |
| `last_scraped` | TIMESTAMPTZ (nullable) | Timestamp of the last successful scrape |
| `failure_count` | INT NOT NULL DEFAULT 0 | Consecutive fetch failures since last successful scrape |
| `last_error_kind` | TEXT (nullable) | Classified failure category (timeout/connect/http status/etc.) |
| `last_status_code` | INT (nullable) | HTTP status code of the last failed fetch |
| `next_retry_at` | TIMESTAMPTZ (nullable) | Earliest timestamp when the URL is eligible for retry |
| `created` / `updated` | TIMESTAMPTZ | |

`shop_urls.state` is crawler-owned URL metadata in Postgres. The scraper updates it after successful normalization and uses it for crawler-side candidate selection. The downstream product backend receives the same normalized availability separately through product upsert commands; that persistence path is related but distinct.

**Domain linkage**: `domain_id` is a direct FK to `shop_domains`. When a domain is removed from a shop during the shop registration sync, all URLs discovered from that domain are automatically cascade-deleted — preventing the scraper from continuing to process stale URLs from a domain that no longer belongs to the shop.

**Change detection**: the scraper computes the current hash in-memory — SHA-256 of the `<main>` fragment when present, SHA-256 of the full HTML otherwise — and compares it with `last_scraped_hash`. If they match and a `<main>` tag was found, extraction is skipped. Pages without a `<main>` tag are always re-extracted.

**Indexes**:
- `idx_shop_urls_class_last_scraped ON shop_urls (url_class, last_scraped)` — supports the scraper candidate query.
- `idx_shop_urls_domain_id ON shop_urls (domain_id)` — supports efficient cascade-delete lookups when a domain is removed.

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

**B-tree index key-size limit**: `raw TEXT PRIMARY KEY` has an implicit B-tree index. PostgreSQL caps B-tree index entries at roughly 2704 bytes. A legitimate state string is at most a few words; any longer text is almost certainly garbage from a misdirected CSS selector. The application enforces `MAX_STATE_RAW_LEN = 512` bytes (in `state_mapping_service.rs`) to reject such inputs before any DB or LLM call — preventing the `INDEX_TOO_LARGE` error that would otherwise cause every scrape of the affected shop to fail permanently.

**Index**: `idx_product_state_mapping_regex ON product_state_mapping (mapping_type) WHERE mapping_type = 'REGEX'` — partial index so the regex scan (`find_all_regex_mappings`) only reads regex rows.

---

## Key Query Patterns

### In-memory locks

The cron job uses an in-memory `LocalLockManager` (`Arc<DashMap<String, Instant>>`) to prevent two concurrent Tokio tasks from processing the same domain or URL simultaneously in the same process.

`DomainLock` uses the `domain_id` UUID XOR-folded to an `i64` key. `UrlLock` uses an FNV-1a hash of the URL string. Both are released automatically when their Rust guard value is dropped.

### Batch upsert into `shop_urls` (UNNEST)

The spider writes up to 100 rows at a time using PostgreSQL `UNNEST` to avoid N individual statements:

```sql
INSERT INTO shop_urls (url, shop_id, domain_id, url_class, state, created, updated)
SELECT $1, $2, t.url, t.url_class, 'UNKNOWN', NOW(), NOW()
FROM UNNEST($3::text[], $4::text[]) AS t(url, url_class)
...
ON CONFLICT (url) DO UPDATE SET
    url_class  = EXCLUDED.url_class,
    domain_id  = EXCLUDED.domain_id,
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
SELECT su.shop_id, su.url, su.last_scraped_hash
FROM   shop_urls su
JOIN   shops s ON s.shop_id = su.shop_id
WHERE  su.url_class = 'product'
  AND  su.state IN ('UNKNOWN', 'LISTED', 'AVAILABLE', 'RESERVED')
  AND  (su.next_retry_at IS NULL OR su.next_retry_at <= NOW())
  AND  (su.last_scraped IS NULL OR su.last_scraped < NOW() - INTERVAL '1 day')
  AND  s.active = TRUE
ORDER BY su.last_scraped NULLS FIRST
LIMIT  $1
```

`SOLD` and `REMOVED` are intentionally excluded from future scrape candidates. `UNKNOWN` remains eligible so newly discovered URLs can be re-scraped until the crawler resolves them to a concrete state.

### Shop registration upsert

The shop sync writes one shop at a time. Shop metadata upsert is followed by a transactional domain sync that uses a bulk `UNNEST` upsert to avoid one query per domain.

```sql
-- Upsert shop metadata
INSERT INTO shops (shop_id, shop_name, shop_slug, active, created, updated)
VALUES ($1, $2, $3, TRUE, NOW(), NOW())
ON CONFLICT (shop_id)
DO UPDATE SET
    shop_name = EXCLUDED.shop_name,
    shop_slug = EXCLUDED.shop_slug,
    active    = TRUE,
    updated   = NOW();

-- Sync domains atomically
BEGIN;

-- Bulk upsert domains for this shop
INSERT INTO shop_domains (shop_id, shop_domain, last_crawled, locked_at)
SELECT $1, domain, NULL, NULL
FROM unnest($2::text[]) AS t(domain)
ON CONFLICT (shop_domain)
DO UPDATE SET
    shop_id = EXCLUDED.shop_id,
    last_crawled = CASE
        WHEN shop_domains.shop_id <> EXCLUDED.shop_id THEN NULL
        ELSE shop_domains.last_crawled
    END,
    locked_at = CASE
        WHEN shop_domains.shop_id <> EXCLUDED.shop_id THEN NULL
        ELSE shop_domains.locked_at
    END;

-- Delete stale domains no longer present for this shop
DELETE FROM shop_domains
WHERE shop_id = $1
  AND NOT (shop_domain = ANY($2::text[]));

COMMIT;

-- Soft-deactivate shops not present in upstream snapshot
UPDATE shops
SET active = FALSE,
    updated = NOW()
WHERE active = TRUE
  AND NOT (shop_id = ANY($3::uuid[]));
```

Domain reassignment is explicit (`shop_id` is updated on conflict), and `last_crawled`/`locked_at` are reset only when ownership changes. Stale domains are removed in the same transaction, and missing shops are soft-deactivated instead of deleted.
