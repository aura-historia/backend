# Crawler — Architecture

The crawler crate has three subsystems — **Shop Registration**, **Spider**, and **Scraper** — driven by a single *
*CronJob**. The spider and scraper are independent of each other at runtime but share the same PostgreSQL database as
the handoff point: the spider writes URLs into `shop_urls`, and the scraper reads them. Shop registration feeds the
`shops` and `shop_domains` tables that the spider depends on.

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
2. Fan the batch out as Tokio worker tasks: one task per spider domain candidate, and one task per scraper domain
   group (all URLs of that domain handled sequentially in that task).
3. Before processing each item, acquire an in-memory lock (`DomainLock` for spider domains, `UrlLock` for scraper URLs)
   via `LocalLockManager`. If another worker already holds the lock the item is skipped (not failed).
4. Call the relevant service (`SpiderService::run` or `ScraperService::scrape`). Errors are logged and swallowed so one
   failure doesn't abort the whole batch.
5. After each batch completes, log a summary line with `total`, `succeeded`, `failed`, `skipped` (lock-skipped items),
   and `duration_ms`. Performance counters are accumulated across batches and a rolling average is emitted every 500
   scraper URLs / every 50 spider domains.

`CrawlerCronJob` carries two `Arc<PerfCounter>` fields for these rolling counters: `scraper_perf` and `spider_perf`.
Each `PerfCounter` encapsulates a count and a cumulative duration, and emits an `info!` summary when its rolling total
reaches the threshold.

The three loops run completely in parallel, each on its own cadence. There is no synchronisation between them beyond the
database.

---

## Shop Registration subsystem

**Goal:** keep the crawler's local `shops` and `shop_domains` tables in sync with the upstream shop service (
OpenSearch), so the spider always has an up-to-date list of shops to crawl.

### Key types — `src/service/shop_registration.rs`

| Type                             | Role                                                                                                                                                                                                         |
|----------------------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `RegisteredShop`                 | Data transfer object: `shop_id`, `shop_name`, `shop_slug`, `domains: HashSet<Domain>`                                                                                                                        |
| `ShopRegistrationSource`         | Trait — fetches all registered shops from an external source. Owned by the crawler crate but **not implemented here**; the concrete implementation lives at the binary level (e.g. `server.rs`).             |
| `ShopRegistrationRepository`     | Trait — persists a `RegisteredShop` into the crawler's Postgres database.                                                                                                                                    |
| `ShopRegistrationService`        | Orchestrator: calls `source.fetch_registered_shops()`, then `repository.upsert_shop()` for each result. Errors on individual upserts are logged and skipped — a single failing shop does not abort the sync. |
| `ShopRegistrationRepositoryImpl` | Postgres-backed implementation of `ShopRegistrationRepository`.                                                                                                                                              |

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

`ShopRegistrationSource` is defined in the crawler crate but its concrete implementation is provided at startup time by
the binary (`server.rs`). This keeps the crawler crate free of any direct dependency on OpenSearch or DynamoDB client
code.

In production (`server.rs`), `OpenSearchShopSource` implements `ShopRegistrationSource` by paginating through
`QueryShopService` (backed by the `shop` crate's OpenSearch repository) until all shops have been fetched.

In tests and the demo binary (`demo.rs`), `DemoShopSource` provides a hardcoded list of `RegisteredShop` values.

### Effect on Spider/Scraper scheduling

- Spider candidates now require `shops.active = TRUE` in addition to the `shop_domains.last_crawled` window.
- Scraper candidates now join `shop_urls` with `shops` and require `shops.active = TRUE`.

This means upstream removals stop both crawling and scraping without deleting historical URL rows.

---

## Spider subsystem

**Goal:** for a given shop, discover every URL on the website and classify each one as `product`, `category`, `imprint`,
`info`, or `other`.

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

Shops that have never been crawled, or were last crawled more than 7 days ago, are eligible. Each candidate carries
`shop_id`, `domain_id`, and `shop_domain`; `domain_id` is threaded through to `SpiderService::run()` so every URL
written to `shop_urls` is linked to the exact domain it was discovered from.

### Optimistic distributed lock (spider-service level)

Before a crawl starts, `SpiderServiceImpl::run` calls `try_lock_shop`, which atomically sets
`shop_domains.locked_at = NOW()` only if the field is currently `NULL` or older than 30 minutes:

```sql
UPDATE shop_domains
SET locked_at = NOW()
WHERE shop_domain = $1
  AND (locked_at IS NULL OR locked_at < NOW() - INTERVAL '30 minutes')
```

If another worker already holds the lock the update affects 0 rows, the service returns an empty `SpiderRunResult`, and
no crawl happens. The lock is released unconditionally at the end of the run (even on error), using a `locked_at = NULL`
update.

### In-memory cron locks (single-process level)

The cron job acquires an in-memory lock before dispatching spider and scraper work, so two concurrent Tokio tasks never
process the same domain or URL at the same time in the same process.

- **`DomainLock`** — acquired per spider candidate using the UUID XOR-folded `i64` key.
- **`UrlLock`** — acquired per scraper candidate using the FNV-1a `i64` hash of the URL.

Both are backed by `LocalLockManager` (`Arc<DashMap<String, Instant>>`) and released automatically by RAII when the
guard is dropped.

### Scraper domain workers

`run_scraper_once` groups candidate URLs by host and spawns one task per domain group (bounded by `scraper_concurrency`
via semaphore). Each domain task:

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

**URL classification** at upsert time uses `CrawledUrl::classify()` — a pure, heuristic function. If a product regex is
known, any URL matching it becomes `product`. Otherwise, keyword matching on the path categorises the URL as `category`,
`imprint`, `info`, or `other` (see `src/spider/utils/url.rs`). The LLM is only called to *find* the regex, not to
classify individual URLs.

**Bloom filter deduplication** in the underlying `spider` crate prevents the same URL from being enqueued more than once
during a single crawl session (100k capacity, 0.1% false-positive rate).

**URL normalisation** strips hash fragments and trailing slashes from every URL before storage or deduplication.

---

## Scraper subsystem

**Goal:** for each known product URL, fetch its page, extract structured product data, and normalise it into a
`NormalizedProduct`.

### Candidate selection — `ScraperCandidateService`

`src/scraper/candidate_service.rs` queries:

```sql
SELECT su.shop_id, su.url, su.last_scraped_hash
FROM shop_urls su
         JOIN shops s ON s.shop_id = su.shop_id
WHERE su.url_class = 'product'
  AND s.llm_calls_count < $2
  AND su.last_scraped_state IN ('UNKNOWN', 'LISTED', 'AVAILABLE', 'RESERVED')
  AND (su.next_retry_at IS NULL OR su.next_retry_at <= NOW())
  AND (su.last_scraped IS NULL OR su.last_scraped < NOW() - INTERVAL '1 day')
  AND s.active = TRUE
ORDER BY su.last_scraped NULLS FIRST
    LIMIT $1
```

Only product URLs for **active** shops that haven't been scraped today, are in an active state, whose retry cooldown has
elapsed, and whose shop-level LLM-call budget (combined across URL classification, schema generation/repair/evaluation,
and state-mapping LLM fallback) is still below cap are eligible.

**Schema seeding on schema-cache miss:** The same service also samples random same-shop product URLs using
`ORDER BY RANDOM() LIMIT ...` while excluding the current URL. This is intentional because schema cache misses are
rare (typically one-time per shop unless schema rows are reset), so this query is not in the hot path. Up to
`scraper_schema_seed_pages - 1` (default 2) additional pages are fetched best-effort; if fetches fail the current page
alone is used to seed the initial schema generation.

**Append-on-miss (runtime schema miss):** When a cached schema variant fails to apply at scrape-time,
`append_single_schema()` generates a single new schema from the current page only — no additional random sampling is
needed. This keeps the append path fast and focused.

### Retry metadata and cooldowns

The scraper now persists network-failure metadata on each URL row:

- `failure_count` — total consecutive fetch failures since the last successful scrape.
- `last_error_kind` — classified failure category (timeout/connect/http status/etc.).
- `last_status_code` — HTTP status when available.
- `next_retry_at` — earliest timestamp when the URL is eligible again.

On each successful scrape (`mark_as_scraped`) these fields are reset. On retryable HTTP failures, the cron worker
records a cooldown (`mark_fetch_failure`) so the URL is skipped until `next_retry_at`. This retry scheduling is
URL-level: one failing product page does not block other URLs from the same domain. The same cooldown path is used for
`SchemaRegenerationExhausted` to prevent repeated LLM-call bursts on a single problematic URL.
The same cooldown path is also used for `LlmBudgetExceeded`: when the per-shop schema-generation budget is exhausted
during an in-flight scrape, a retry cooldown is written for observability. In steady state, hard-stop is enforced
earlier by candidate selection (`shops.llm_calls_count < cap`).

The scraper also has a lightweight in-batch domain politeness layer. Retryable URL failures and adaptive domain delay
are intentionally separate decisions:

- `is_retryable_network_failure(...)` controls whether the failed URL gets `next_retry_at`.
- `should_adapt_domain_delay(...)` controls whether the current domain worker slows down before processing the next URL
  from that same domain.

Domain delay adapts only for signals that suggest domain-wide pressure or availability trouble: HTTP `408`, `429`,
`503`, `504`, request `Timeout`, and `Connect` failures. It does not adapt for retryable failures that are more likely
to be URL/request-scoped, such as HTTP `500`, `502`, `425`, or generic `Request` failures. For example, a `500` writes
`next_retry_at` for that URL and the same-domain batch continues at the normal pace; a `429` writes URL retry metadata
and increases the in-memory delay between remaining same-domain URLs.

Adaptive domain delay is process-local and batch-local. It starts from `scraper_domain_delay`, doubles after each
domain-delay signal, is capped at 10 seconds, and decays after five clean requested outcomes. It is deliberately not
persisted to `shop_domains`, so it is a short-lived politeness mechanism rather than a domain-wide scheduling lock.

### Scrape execution — `ScraperServiceImpl`

`src/scraper/scraper_service.rs`:

```
scrape(shop_id, url, last_scraped_hash)
 ├── HtmlFetcher::fetch(url)                  — download raw HTML
 ├── current_hash = SHA-256(<main> fragment) if present, else SHA-256(full HTML)
 ├── if <main> present AND current_hash == last_scraped_hash
 │    └── touch_scraped(current_hash) and return None      — page unchanged, skip extraction
 ├── ProductSchemaService::obtain_schemas(shop_id, html)
 │    ├── DB hit  → return cached CSS selector schema set (Vec of variants)
 │    └── DB miss → seed pages (current + up to N-1 random same-shop)
 │         └── LLM generates schema set (single call, may return multiple)
 │              → persist → return
 ├── try cached schema variants in order
 │    ├── first applicable variant → RawExtractedProduct
 │    └── none applies → [append-and-retry loop]
	 │         ├── fixed prompt-source attempts: YAML projection, then cleaned HTML fallback
	 │         │    ├── attempt 1: LLM generates ONE schema from the current page YAML projection
	 │         │    ├── attempt 2: LLM gets cleaned HTML plus previous failed schema + extraction error
 │         │    ├── in-memory candidate = existing schemas + generated schema
 │         │    ├── re-apply only schemas not already known to fail in this loop
 │         │    ├── if one applies → dedupe schema set, persist, continue
 │         │    └── if none apply → discard generated schema and retry
 │         └── if attempts exhausted → return SchemaRegenerationExhausted
 ├── ProductNormalizationService::normalize(raw, url)
 │    ├── state: ProductStateMappingService::get_state_mapping(raw.state)
 │    │    ├── [guard] len > MAX_STATE_RAW_LEN (512 bytes)?
 │    │    │    └── warn + return StateTextTooLong
 │    │    ├── exact DB lookup   (e.g. "sold" → SOLD)
 │    │    ├── regex DB scan     (e.g. "3 left" matches \b[1-9]...\bleft\b → AVAILABLE)
 │    │    └── LLM fallback → persist result for future lookups
      │    ├── shops_product_id: if extracted value is blank, falls back to the full URL (infallible)
      │    ├── title: detect language (lingua), wrap in Localized<Title>
      │    ├── price: parse currency + amount (multi-locale)
      │    ├── images: resolve relative URLs against page URL
      │    └── dates: parse ISO 8601 / RFC 3339
      │    └── normalization error
      │         ├── fixable selector-type errors (title/price/state-text-too-long) → append-and-retry schema regeneration (budget-guarded)
      │         └── all other errors → propagate as normalization failure
 ├── set_state(shop_id, url, normalized_state) → updates shop_urls.last_scraped_state
 └── mark_as_scraped is done by caller after successful product push
```

On schema cache miss, scraper schema generation can include multiple seed pages (current page + up to `N-1` additional
same-shop product pages, best-effort). These seed pages are sent in one LLM call that may return multiple schema
variants for heterogeneous templates. This improves first schema quality, but first scrape latency can increase due to
additional fetches on that one-time path.

Schema generation is now gated before persistence:

```
generated schemas + sampled pages
  -> deterministic schema application matrix
  -> optional judge-only LLM evaluation
  -> auto-approve only when deterministic checks pass and evaluator returns APPROVE/HIGH
  -> otherwise create pending PRODUCT_SCHEMA review
```

When `CRAWLER_SCHEMA_LLM_REVIEW_MODE` is `report_only` or `auto_approve_high_confidence`, the generated schemas are
passed to the evaluator together with cleaned HTML from the in-memory crawl context and extraction evidence. The verdict
is stored in
`crawler_reviews.validation_summary.auto_schema_evaluation` and displayed in the Crawler Review Console. Rejections, low
confidence, malformed responses, LLM errors, or evaluator budget exhaustion all create the normal pending review.

Review page HTML is not persisted. `crawler_review_pages` stores URL, role, and the original HTML hash only. The Crawler
Review Console fetches live HTML from the stored URL for inspector, raw HTML view, and manual matrix re-evaluation, so
those views reflect the current shop page rather than an immutable historical snapshot.

Auto-approved schema reviews remain editable in the console. Editing a field, schema order, added/deleted schema, or the
full JSON on an approved `PRODUCT_SCHEMA` review updates the live `shops_product_schema` row immediately, refreshes the
review's candidate payload for audit readability, and appends a `manual_schema_edits` entry to
`crawler_reviews.validation_summary`.

**Append-on-miss flow** — triggered when no cached schema variant applies during scrape:

```
[append-on-miss flow]
 ├── ProductSchemaService::append_single_schema(domain, html, failed_schema?, last_error?)
	 │    ├── attempt 1: LLM generates a single schema from the YAML projection
	 │    ├── attempt 2: LLM receives cleaned HTML plus previous failed generated schema + extraction error
 │    │        Prompt emphasizes: "single schema for one page, for append/retry"
 │    ├── append to existing variant set in memory
 │    └── return expanded ShopsProductSchema candidate
 │
 ├── retry only newly appended schemas (exclude known failed by content)
 │    ├── matches now? → persist expanded set and continue
	 │    └── still no match → discard generated schema and try the next prompt source
```

This enables heterogeneous shops (with multiple page layouts) to dynamically accumulate schema variants without full
regeneration. Only applicable generated schemas are persisted; non-applicable candidates are discarded. Before
persistence, schemas are deduplicated.

**`scraper::Html` is `!Send`**: the parsed HTML object cannot be held across an `.await`. `apply_schema()` is a
synchronous helper that parses the HTML, applies the schema, and returns — ensuring no `Html` value is live when any
`.await` point is reached.

**Attempt budget and observability**: the append-and-retry loop uses the fixed YAML projection then cleaned HTML
fallback
sequence. When both attempts are exhausted, scraping returns `SchemaRegenerationExhausted`, cron persists the failure
and
writes a cooldown (`next_retry_at`) so the URL is skipped for a backoff window. Every schema-generation LLM call
increments
`shops.llm_calls_count`.

**Hard LLM budget stop**: shop-scoped LLM calls are tracked in `shops.llm_calls_count` (URL pattern classification +
schema generation/repair/evaluation + state-mapping LLM fallback). All crawler LLM call types share a single combined
cap (`scraper_max_llm_calls_per_shop`, default `20`). Once the cap is reached, scraper candidate selection excludes that
shop entirely (`shops.llm_calls_count < cap`), preventing subsequent scrape loops. If a scrape hits the cap mid-run,
scraper returns `LlmBudgetExceeded`; cron records cooldown metadata via `mark_fetch_failure`. If only the schema
evaluator hits the cap, the schema is routed to pending human review instead of being auto-approved.

State-mapping LLM calls are charged post-hoc: `normalize()` returns `(NormalizedProduct, u32)` where the `u32` is the
number of LLM calls used (0 when the state was resolved via DB lookup, 1 when the LLM fallback was invoked). The
caller (`normalize_with_schema_fix_retry` in `scraper_service.rs`) charges
`consume_llm_budget_n_or_err(shop_id, url, n)` after each normalize call; the function is a no-op when `n == 0`.

---

## How Spider and Scraper Connect

The two subsystems communicate exclusively through `shop_urls`:

```
Spider writes:
  INSERT INTO shop_urls (url, shop_id, domain_id, url_class, last_scraped_state, ...)
  ON CONFLICT (url) DO UPDATE SET url_class = ..., domain_id = ..., updated = NOW()

Scraper reads:
  SELECT ... FROM shop_urls WHERE url_class = 'product' AND ...

Scraper writes back:
  UPDATE shop_urls SET last_scraped_state = $state, updated = NOW()
  UPDATE shop_urls SET last_scraped_hash = $hash, last_scraped = NOW()
  UPDATE shop_urls SET state = 'REMOVED', updated = NOW()
```

The spider decides *which* URLs are products (by running the LLM-found regex). The scraper only processes URLs the
spider has already labeled as `url_class = 'product'`. There is no direct function call or shared in-memory state
between them — just the database row.

Change detection is scraper-local: after fetching HTML, the scraper computes a hash in-memory — SHA-256 of the `<main>`
fragment when present, or SHA-256 of the full HTML when there is no `<main>` tag. This hash is compared to
`last_scraped_hash`. If they match (and a `<main>` tag was found), extraction is skipped. Pages without a `<main>` tag
are always re-extracted. The stored hash is always written after a successful scrape regardless of whether a `<main>`
tag was present.

The crawler now also tracks crawl-level cooldown metadata on `shop_domains`:

- `crawl_failure_count`
- `last_crawl_error_kind`
- `next_crawl_at`

Spider candidate selection excludes domains with `next_crawl_at > NOW()`. On a successful crawl the metadata is reset.
On failure, cron compares the new error kind with `last_crawl_error_kind`: matching kinds increment
`crawl_failure_count`; changed kinds reset it to `1`. Pending URL-pattern reviews and generic spider failures keep the
existing short retry cooldown.

For zero/one-page crawls, spider preflight diagnostics can persist a more specific failure kind before falling back to
`EmptyCrawl` or `TinyCrawl`: `RateLimited`, `AccessDenied`, `CloudflareChallenge`, `TlsError`, `RobotsBlocked`,
`RedirectProblem`, or `JavascriptRequired`. Diagnostics describe the likely cause; cooldown groups control retry
pressure:

| Failure group | Error kinds | Attempts 1-2 | Attempt 3+ |
| --- | --- | --- | --- |
| Transient/flaky | `EmptyCrawl`, `TinyCrawl`, `RateLimited`, `CloudflareChallenge` | 6 hours | 24 hours |
| Recoverable site/config or low-confidence sample | `InsufficientInferenceSample`, `TlsError`, `RedirectProblem` | 6 hours | 3 days |
| Durable block | `AccessDenied`, `RobotsBlocked`, `JavascriptRequired` | 6 hours | 30 days |
