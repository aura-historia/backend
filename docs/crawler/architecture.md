# Crawler — Architecture

The crawler crate has three subsystems — **Shop Registration**, **Spider**, and **Scraper** — driven by a single **CronJob**. The spider and scraper are independent of each other at runtime but share the same PostgreSQL database as the handoff point: the spider writes URLs into `shop_urls`, and the scraper reads them. Shop registration feeds the `shops` and `shop_domains` tables that the spider depends on.

---

## CrawlerCronJob — the driver

`src/service/cron.rs` is the entry point. On startup it spawns three independent `tokio` tasks that loop forever:

```
CrawlerCronJob::run_loop()
  ├── tokio::spawn → shop_sync_loop  (runs immediately, then sleeps 3 h between ticks)
  ├── tokio::spawn → spider_loop     (sleeps 10 min between ticks)
  └── tokio::spawn → scraper_loop    (sleeps 1 min between ticks)
```

The spider and scraper loops follow the same pattern each tick:

1. Ask the relevant **CandidateService** for a batch of work.
2. Fan the batch out as Tokio worker tasks: one task per spider domain candidate, and one task per scraper domain group (all URLs of that domain handled sequentially in that task).
3. Before processing each item, acquire an in-memory lock (`DomainLock` for spider domains, `UrlLock` for scraper URLs) via `LocalLockManager`. If another worker already holds the lock the item is skipped (not failed).
4. Call the relevant service (`SpiderService::run` or `ScraperService::scrape`). Errors are logged and swallowed so one failure doesn't abort the whole batch.
5. After each batch completes, log a summary line with `total`, `succeeded`, `failed`, `skipped` (lock-skipped items), and `duration_ms`. Performance counters are accumulated across batches and a rolling average is emitted every 500 scraper URLs / every 50 spider domains.

`CrawlerCronJob` carries two `Arc<PerfCounter>` fields for these rolling counters: `scraper_perf` and `spider_perf`. Each `PerfCounter` encapsulates a count and a cumulative duration, and emits an `info!` summary when its rolling total reaches the threshold.

The three loops run completely in parallel, each on its own cadence. There is no synchronisation between them beyond the database.

---

## Shop Registration subsystem

**Goal:** keep the crawler's local `shops` and `shop_domains` tables in sync with the upstream shop service (OpenSearch), so the spider always has an up-to-date list of shops to crawl.

### Key types — `src/service/shop_registration.rs`

| Type | Role |
|------|------|
| `RegisteredShop` | Data transfer object: `shop_id`, `shop_name`, `shop_slug`, `domains: HashSet<Domain>` |
| `ShopRegistrationSource` | Trait — fetches all registered shops from an external source. Owned by the crawler crate but **not implemented here**; the concrete implementation lives at the binary level (e.g. `server.rs`). |
| `ShopRegistrationRepository` | Trait — persists a `RegisteredShop` into the crawler's Postgres database. |
| `ShopRegistrationService` | Orchestrator: calls `source.fetch_registered_shops()`, then `repository.upsert_shop()` for each result. Errors on individual upserts are logged and skipped — a single failing shop does not abort the sync. |
| `ShopRegistrationRepositoryImpl` | Postgres-backed implementation of `ShopRegistrationRepository`. |

### Sync loop

```
shop_sync_loop()
  ├── run_shop_sync_once()    ← executes immediately on startup
  └── sleep(shop_sync_interval)  ← default 3 hours
      └── repeat
```

`run_shop_sync_once()` delegates entirely to `ShopRegistrationService::sync()`:

```
ShopRegistrationService::sync()
  ├── source.fetch_registered_shops()   → Vec<RegisteredShop>
  ├── if empty result:
  │    └── warn + return (skip deactivation to avoid accidental mass-disable)
  └── for each shop:
       ├── repository.upsert_shop(shop)
       │    └── INSERT INTO shops ... ON CONFLICT DO UPDATE SET shop_name, shop_slug, active=TRUE, updated
       ├── repository.sync_domains(shop)
       │    ├── begin transaction
       │    ├── bulk upsert domains via UNNEST
       │    ├── on reassignment only: reset last_crawled + locked_at
       │    ├── delete stale domains no longer present upstream for this shop
       │    └── commit transaction
       └── after all shops:
            └── repository.deactivate_shops_not_in(all_fetched_shop_ids)
                 └── UPDATE shops SET active=FALSE for shops absent upstream
```

### Decoupling via trait injection

`ShopRegistrationSource` is defined in the crawler crate but its concrete implementation is provided at startup time by the binary (`server.rs`). This keeps the crawler crate free of any direct dependency on OpenSearch or DynamoDB client code.

In production (`server.rs`), `OpenSearchShopSource` implements `ShopRegistrationSource` by paginating through `QueryShopService` (backed by the `shop` crate's OpenSearch repository) until all shops have been fetched.

In tests and the demo binary (`demo.rs`), `DemoShopSource` provides a hardcoded list of `RegisteredShop` values.

### Effect on Spider/Scraper scheduling

- Spider candidates now require `shops.active = TRUE` in addition to the `shop_domains.last_crawled` window.
- Scraper candidates now join `shop_urls` with `shops` and require `shops.active = TRUE`.

This means upstream removals stop both crawling and scraping without deleting historical URL rows.

---

## Spider subsystem

**Goal:** for a given shop, discover every URL on the website and classify each one as `product`, `category`, `imprint`, `info`, or `other`.

### Candidate selection — `SpiderCandidateService`

`src/spider/candidate_service.rs` queries:

```sql
SELECT s.shop_id, sd.domain_id, sd.shop_domain
FROM shops s
JOIN shop_domains sd ON sd.shop_id = s.shop_id
WHERE sd.last_crawled IS NULL
   OR sd.last_crawled < NOW() - INTERVAL '7 days'
LIMIT $1
```

Shops that have never been crawled, or were last crawled more than 7 days ago, are eligible. Each candidate carries `shop_id`, `domain_id`, and `shop_domain`; `domain_id` is threaded through to `SpiderService::run()` so every URL written to `shop_urls` is linked to the exact domain it was discovered from.

### Optimistic distributed lock (spider-service level)

Before a crawl starts, `SpiderServiceImpl::run` calls `try_lock_shop`, which atomically sets `shop_domains.locked_at = NOW()` only if the field is currently `NULL` or older than 30 minutes:

```sql
UPDATE shop_domains
SET    locked_at = NOW()
WHERE  shop_domain = $1
  AND  (locked_at IS NULL OR locked_at < NOW() - INTERVAL '30 minutes')
```

If another worker already holds the lock the update affects 0 rows, the service returns an empty `SpiderRunResult`, and no crawl happens. The lock is released unconditionally at the end of the run (even on error), using a `locked_at = NULL` update.

### In-memory cron locks (single-process level)

The cron job acquires an in-memory lock before dispatching spider and scraper work, so two concurrent Tokio tasks never process the same domain or URL at the same time in the same process.

- **`DomainLock`** — acquired per spider candidate using the UUID XOR-folded `i64` key.
- **`UrlLock`** — acquired per scraper candidate using the FNV-1a `i64` hash of the URL.

Both are backed by `LocalLockManager` (`Arc<DashMap<String, Instant>>`) and released automatically by RAII when the guard is dropped.

### Scraper domain workers

`run_scraper_once` groups candidate URLs by host and spawns one task per domain group (bounded by `scraper_concurrency` via semaphore). Each domain task:

1. Scrapes that domain's URLs sequentially.
2. Applies `scraper_domain_delay` between consecutive URLs for that domain.
3. Returns commands/counts to the caller for batched push.

This keeps the scheduling logic simple while preserving per-domain pacing and multi-domain parallelism.

### Crawl execution — `SpiderServiceImpl`

`src/spider/service/spider_service.rs` orchestrates the crawl:

```
run(shop_domain, domain_id)
 ├── try_lock_shop()               — acquire optimistic lock
 ├── Spider::crawl(shop_url)       — returns mpsc::Receiver<CrawledPage>
 ├── load_pattern_for_shop()       — load persisted regex from shops.url_pattern (if any)
 │
 │   [stream loop — one iteration per CrawledPage received]
 ├── push URL to inference_sample  (capped at 500)
 ├── push page to page_buffer
 ├── if total_crawled >= classify_threshold (200):
 │    └── LLM: classify_and_save() → persist regex to shops.url_pattern
 ├── if buffer full (100 pages) AND classification done:
 │    └── batch-upsert page_buffer → shop_urls (UNNEST)
 │
 │   [after stream ends]
 ├── if not yet classified → classify now (small shops never hit threshold)
 ├── if persisted pattern matched 0 products → reclassify (stale pattern)
 ├── flush remaining page_buffer → shop_urls
 ├── mark_as_crawled() → sets shop_domains.last_crawled = NOW()
 └── unlock_shop()                 — release lock
```

**URL classification** at upsert time uses `CrawledUrl::classify()` — a pure, heuristic function. If a product regex is known, any URL matching it becomes `product`. Otherwise, keyword matching on the path categorises the URL as `category`, `imprint`, `info`, or `other` (see `src/spider/utils/url.rs`). The LLM is only called to *find* the regex, not to classify individual URLs.

**Bloom filter deduplication** in the underlying `spider` crate prevents the same URL from being enqueued more than once during a single crawl session (100k capacity, 0.1% false-positive rate).

**URL normalisation** strips hash fragments and trailing slashes from every URL before storage or deduplication.

---

## Scraper subsystem

**Goal:** for each known product URL, fetch its page, extract structured product data, and normalise it into a `NormalizedProduct`.

### Candidate selection — `ScraperCandidateService`

`src/scraper/candidate_service.rs` queries:

```sql
SELECT su.shop_id, su.url, su.last_scraped_hash
FROM   shop_urls su
JOIN   shops s ON s.shop_id = su.shop_id
WHERE  su.url_class  = 'product'
  AND  su.state      IN ('UNKNOWN', 'LISTED', 'AVAILABLE', 'RESERVED')
  AND  (su.next_retry_at IS NULL OR su.next_retry_at <= NOW())
  AND  (su.last_scraped IS NULL OR su.last_scraped < NOW() - INTERVAL '1 day')
  AND  s.active = TRUE
ORDER BY su.last_scraped NULLS FIRST
LIMIT  $1
```

Only product URLs for **active** shops that haven't been scraped today, are in an active state, and whose retry cooldown has elapsed are eligible.

### Retry metadata and cooldowns

The scraper now persists network-failure metadata on each URL row:

- `failure_count` — total consecutive fetch failures since the last successful scrape.
- `last_error_kind` — classified failure category (timeout/connect/http status/etc.).
- `last_status_code` — HTTP status when available.
- `next_retry_at` — earliest timestamp when the URL is eligible again.

On each successful scrape (`mark_as_scraped`) these fields are reset. On retryable HTTP failures, the cron worker records a cooldown (`mark_fetch_failure`) so the URL is skipped until `next_retry_at`.

### Scrape execution — `ScraperServiceImpl`

`src/scraper/scraper_service.rs`:

```
scrape(shop_id, url, last_scraped_hash)
 ├── HtmlFetcher::fetch(url)                  — download raw HTML
 ├── current_hash = SHA-256(<main> fragment) if present, else SHA-256(full HTML)
 ├── if <main> present AND current_hash == last_scraped_hash
 │    └── mark_as_scraped(current_hash) and return None   — page unchanged, skip extraction
 ├── ProductSchemaService::get_product_schema(shop_id, html)
 │    ├── DB hit  → return cached CSS selector schema
 │    └── DB miss → LLM generates schema → persist → return
 ├── schema.apply(Html::parse_document(&html))  → RawExtractedProduct
 │    └── fails → [schema-fix flow A] (see below)
 ├── ProductNormalizationService::normalize(raw, url)
 │    ├── state: ProductStateMappingService::get_state_mapping(raw.state)
 │    │    ├── [guard] len > MAX_STATE_RAW_LEN (512 bytes)?
 │    │    │    └── warn + return StateTextTooLong → triggers schema-fix flow B
 │    │    ├── exact DB lookup   (e.g. "sold" → SOLD)
 │    │    ├── regex DB scan     (e.g. "3 left" matches \b[1-9]...\bleft\b → AVAILABLE)
 │    │    └── LLM fallback → persist result for future lookups
    │    ├── shops_product_id: if extracted value is blank, falls back to the full URL (infallible)
    │    ├── title: detect language (lingua), wrap in Localized<Title>
    │    ├── price: parse currency + amount (multi-locale)
    │    ├── images: resolve relative URLs against page URL
    │    └── dates: parse ISO 8601 / RFC 3339
    │    └── other normalization error (price/title bad) → [schema-fix flow B]
 ├── set_state(shop_id, url, normalized_state) → updates shop_urls.state
 ├── mark_as_scraped(shop_id, url, current_hash) → updates hash/timestamp bookkeeping
 └── if schema was fixed this run → reset_fix_attempts(domain)
```

**Schema-fix flow A** — triggered when `schema.apply()` fails:

```
[schema-fix flow A]
 ├── is_fix_budget_exhausted(domain)?  → bail with SchemaFixAttemptsExhausted
 ├── increment_fix_attempts(domain)
 ├── LLM: fix_product_schema(failed_schema, apply_error, html)
 ├── re-apply fixed schema
 │    ├── ok → persist fixed schema → save_product_schema()
 │    │        schema_was_fixed = true
 │    └── fails → return SchemaFixApplyFailed (not persisted)
```

The dispatcher (`cron.rs`) guarantees at most one in-flight scrape per domain at a time, so no per-domain mutex is needed inside the fix path.

**Schema-fix flow B** — triggered when normalization returns a schema-fixable error (bad state selector text, price parse failure, empty title, etc.).  `ShopsProductIdEmpty` is **not** schema-fixable and is propagated directly as `ScraperError::NormalizationError` without calling the LLM. `PriceUnknownCurrency` **is** schema-fixable for genuinely unknown currency-bearing price text — the synthetic hint asks the LLM to add a `default_currency` to the schema. Explicit non-numeric markers like `"Price on Request"` are normalized to `price=None` and do not enter the fix loop:

```
[schema-fix flow B — normalize_with_retry]
 ├── normalization_error_to_schema_hint(err) → Option<ApplySchemaError>
 │    None  → propagate NormalizationError (image URL errors, etc.)
 │    Some  → proceed with hint_error as synthetic apply error
 │
 │   Fix attempt 1: fix_and_apply_schema(schema, hint_error, html)
 │    ├── schema_was_fixed = true if LLM fix applied
 │    └── normalize(fixed_raw)
 │         ├── ok → done
 │         └── fails →
 │              Fix attempt 2: fix_and_apply_schema(schema, hint_error, html)
 │               └── normalize(final_raw)
 │                    ├── ok → done
 │                    └── fails → propagate NormalizationError
```

**`scraper::Html` is `!Send`**: the parsed HTML object cannot be held across an `.await`. `apply_schema()` is a synchronous helper that parses the HTML, applies the schema, and returns — ensuring no `Html` value is live when any `.await` point is reached.

**Per-domain fix-attempt tracking**: `schema_fix_attempts: Arc<Mutex<HashMap<String, u32>>>` records how many *consecutive* LLM-fix attempts have failed end-to-end for each domain. Once the count reaches `max_schema_fix_attempts` the domain returns `SchemaFixAttemptsExhausted` without calling the LLM, preventing infinite fix loops. The counter is reset to zero after **every** successful scrape for the domain (with or without a fix), so it measures failures since the last clean scrape rather than total lifetime failures. This prevents premature budget exhaustion on domains whose pages have heterogeneous layouts.

---

## How Spider and Scraper Connect

The two subsystems communicate exclusively through `shop_urls`:

```
Spider writes:
  INSERT INTO shop_urls (url, shop_id, domain_id, url_class, state, ...)
  ON CONFLICT (url) DO UPDATE SET url_class = ..., domain_id = ..., updated = NOW()

Scraper reads:
  SELECT ... FROM shop_urls WHERE url_class = 'product' AND ...

Scraper writes back:
  UPDATE shop_urls SET state = $state, updated = NOW()
  UPDATE shop_urls SET last_scraped_hash = $hash, last_scraped = NOW()
  UPDATE shop_urls SET state = 'REMOVED', updated = NOW()
```

The spider decides *which* URLs are products (by running the LLM-found regex). The scraper only processes URLs the spider has already labelled as `url_class = 'product'`. There is no direct function call or shared in-memory state between them — just the database row.

Change detection is scraper-local: after fetching HTML, the scraper computes a hash in-memory — SHA-256 of the `<main>` fragment when present, or SHA-256 of the full HTML when there is no `<main>` tag. This hash is compared to `last_scraped_hash`. If they match (and a `<main>` tag was found), extraction is skipped. Pages without a `<main>` tag are always re-extracted. The stored hash is always written after a successful scrape regardless of whether a `<main>` tag was present.

The crawler now also tracks crawl-level cooldown metadata on `shop_domains`:

- `crawl_failure_count`
- `last_crawl_error_kind`
- `next_crawl_at`

Spider candidate selection excludes domains with `next_crawl_at > NOW()`. On a successful crawl the metadata is reset; on failure a cooldown is written to defer the next attempt.
