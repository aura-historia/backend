# Configuration

The crawler is configured via two structs: `CrawlerCronConfig` (top-level, controls all three tasks) and
`SpiderServiceConfig` (spider-specific tuning).

---

## `CrawlerCronConfig`

Controls the three background tasks spawned by `CrawlerCronJob`.

| Field                            | Type          | Default       | Description                                                                                                                                                                                                                                                                                                                                                                            |
|----------------------------------|---------------|---------------|----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `shop_sync_interval`             | `Duration`    | 3 hours       | How often the shop registration sync task wakes to fetch shops from the upstream service and upsert them into the local DB. The task also runs once immediately on startup.                                                                                                                                                                                                            |
| `spider_interval`                | `Duration`    | 10 min        | How often the spider task wakes to select and crawl candidate shops                                                                                                                                                                                                                                                                                                                    |
| `scraper_interval`               | `Duration`    | 1 min         | How often the scraper task wakes to select and scrape candidate URLs                                                                                                                                                                                                                                                                                                                   |
| `spider_batch_size`              | `i64`         | 10            | Max shops selected per spider tick (`LIMIT` in candidate query)                                                                                                                                                                                                                                                                                                                        |
| `scraper_batch_size`             | `i64`         | 100           | Max URLs selected per scraper tick                                                                                                                                                                                                                                                                                                                                                     |
| `push_batch_size`                | `usize`       | 25            | Number of scraped products accumulated before flushing a push to the backend. Keeps memory bounded and avoids holding all results until the last scrape finishes.                                                                                                                                                                                                                      |
| `spider_concurrency`             | `usize`       | 3             | Max concurrent spider domain worker tasks per tick                                                                                                                                                                                                                                                                                                                                     |
| `scraper_concurrency`            | `usize`       | 10            | Max concurrent scraper domain worker tasks per tick                                                                                                                                                                                                                                                                                                                                    |
| `scraper_schema_seed_pages`      | `usize`       | 3             | Number of product pages used to seed initial schema generation on a schema cache miss. The scraper always includes the current page and then samples up to `N-1` extra same-shop product URLs; all seed pages are sent in a single LLM call that may return multiple schema variants.                                                                                                  |
| `scraper_max_llm_calls_per_shop` | `i64`         | 20            | Hard per-shop cap used by scraper cost guardrails. The cap is checked against `shops.llm_calls_count` (shop-scoped LLM counter: URL pattern classification, schema generation/repair/evaluation, and state mapping fallback). Once reached, the scraper candidate query excludes that shop entirely (`shops.llm_calls_count < cap`) so subsequent scrapes are blocked for cost safety. |
| `spider_classify_threshold`      | `usize`       | 200           | Passed through to `SpiderServiceConfig::classify_threshold` — number of URLs buffered before triggering mid-run LLM URL classification                                                                                                                                                                                                                                                 |
| `db_max_connections`             | `Option<u32>` | `None` (auto) | Maximum Postgres connections in the pool. Locks are now in-memory (`LocalLockManager`), so this setting mainly controls query capacity. When `None`, auto-computed as `spider_concurrency + scraper_concurrency + 10`.                                                                                                                                                                 |

---

## `SpiderServiceConfig`

Controls per-run behavior of the spider.

| Field                       | Type       | Default | Description                                                                                                                              |
|-----------------------------|------------|---------|------------------------------------------------------------------------------------------------------------------------------------------|
| `classify_threshold`        | `usize`    | 200     | Number of URLs buffered before triggering mid-run LLM URL classification. Populated from `CrawlerCronConfig::spider_classify_threshold`. |
| `db_batch_size`             | `usize`    | 100     | Number of `shop_urls` rows flushed per UNNEST batch upsert                                                                               |
| `max_sample_urls`           | `usize`    | 500     | Max URLs sent to the URL classification LLM                                                                                              |
| `min_inference_sample_urls` | `usize`    | 20      | Minimum crawled URL sample required before URL pattern inference or refresh can spend an LLM call                                        |
| `lock_timeout`              | `Duration` | 30 min  | How long a `shop_domains.locked_at` lock is considered valid before it can be overridden (spider-service-level optimistic lock)          |
| `bloom_capacity`            | `usize`    | 100_000 | Bloom filter capacity (max unique URLs tracked for dedup per crawl run)                                                                  |
| `bloom_fp_rate`             | `f64`      | 0.001   | Bloom filter false-positive rate                                                                                                         |

---

## Environment Variables

| Variable                         | Values                                                      | Default                                                                                    | Description                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
|----------------------------------|-------------------------------------------------------------|--------------------------------------------------------------------------------------------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `CRAWLER_SCHEMA_LLM_REVIEW_MODE` | `human_only`, `report_only`, `auto_approve_high_confidence` | `auto_approve_high_confidence` when `CRAWLER_REVIEW_REQUIRED=true`; otherwise `human_only` | Controls the judge-only LLM schema evaluation gate. `human_only` skips evaluator calls and creates the normal human review. `report_only` stores evaluator decision/confidence/rationale in the review but never auto-approves. `auto_approve_high_confidence` persists schemas only when deterministic application passes and the evaluator returns `APPROVE` with `HIGH` confidence; all other outcomes create pending human reviews. Evaluator calls count against `scraper_max_llm_calls_per_shop`. |

---

## Interactions Between Settings

- **`shop_sync_interval`**: The sync runs once at startup regardless of this value, so a newly deployed instance always
  has an up-to-date shop list before the spider's first tick. Set this shorter than `spider_interval` if you need faster
  shop discovery; the default 3-hour cadence is sufficient for typical onboarding flows.

- **`classify_threshold` vs `max_sample_urls`**: Classification is triggered at `classify_threshold` URLs buffered, but
  only up to `max_sample_urls` are sent to the LLM. If more URLs arrive before the threshold is hit, they are still
  buffered (for DB persistence) but only the first `max_sample_urls` are included in the LLM prompt.
  `spider_classify_threshold` in `CrawlerCronConfig` is passed directly to `SpiderServiceConfig::classify_threshold` at
  construction time.

- **`min_inference_sample_urls`**: URL pattern inference is skipped when the crawl sample has fewer than this many URLs,
  preventing low-confidence LLM calls on tiny crawls. Crawls with one URL or less are treated as failed crawls; crawls
  with 2-19 URLs complete without URL pattern inference and persist metadata with the current or unknown pattern.

- **`spider_batch_size` + `spider_concurrency`**: At each tick, up to `spider_batch_size` shops are selected and up to
  `spider_concurrency` are crawled concurrently. If a domain's in-memory lock (`DomainLock`) is held by another task the
  item is skipped immediately (counted as `skipped` in the batch log) rather than failing.

- **`scraper_batch_size` + `scraper_concurrency` + `scraper_domain_delay`**: The scraper tick selects up to
  `scraper_concurrency * scraper_batch_size` URLs, groups them by domain, and runs one worker task per domain (bounded
  by `scraper_concurrency`). Each domain worker processes its URLs sequentially and sleeps `scraper_domain_delay`
  between URLs for that domain.

- **`lock_timeout`**: Prevents a crashed spider run from blocking a shop indefinitely via the `shop_domains.locked_at`
  optimistic lock. If `locked_at` is older than `lock_timeout`, the lock is treated as expired and can be acquired by
  the next run. The cron-level in-memory lock is process-local and released when the process exits.

- **`db_max_connections`**: With in-memory locks, pool usage is dominated by repository queries instead of long-lived
  lock sessions. The auto-computed value (`spider_concurrency + scraper_concurrency + 10`) keeps comfortable headroom
  for the spider/scraper/query mix.

- **`push_batch_size`**: Controls how often the scraper loop flushes scraped products to the backend push service.
  Smaller values reduce peak memory at the cost of more push calls; larger values amortise push overhead but hold more
  results in memory.

- **`db_batch_size`**: Tune this based on Postgres UNNEST performance. Larger batches reduce round-trips but increase
  per-statement memory. Default of 100 is conservative.

- **`scraper_schema_seed_pages`**: Increasing this improves first-time schema quality by sampling more page layouts, but
  first scrape latency can increase because each schema cache miss may perform up to `N-1` additional HTTP fetches.
  These extra fetches are best-effort and only occur on cache miss. The resulting single LLM call can return multiple
  schema variants for heterogeneous templates.

- **`scraper_max_llm_calls_per_shop`**: Cost safety hard-stop. Once a shop reaches the cap, scraper candidate selection
  no longer returns its URLs, preventing further scrape loops and additional LLM spend for that shop. Schema evaluator
  calls use the same budget; if the budget is exhausted, the generated schema falls back to a pending human review.

- **`CRAWLER_SCHEMA_LLM_REVIEW_MODE`**: Use `report_only` to collect evaluator confidence and rationale without changing
  approval behavior. Use `auto_approve_high_confidence` when operators are comfortable letting high-confidence,
  deterministically passing schemas go live while preserving an approved audit review in the console.
