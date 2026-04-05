# Configuration

The crawler is configured via two structs: `CrawlerCronConfig` (top-level, controls all three tasks) and `SpiderServiceConfig` (spider-specific tuning).

---

## `CrawlerCronConfig`

Controls the three background tasks spawned by `CrawlerCronJob`.

| Field | Type | Default | Description |
|---|---|---|---|
| `shop_sync_interval` | `Duration` | 3 hours | How often the shop registration sync task wakes to fetch shops from the upstream service and upsert them into the local DB. The task also runs once immediately on startup. |
| `spider_interval` | `Duration` | 10 min | How often the spider task wakes to select and crawl candidate shops |
| `scraper_interval` | `Duration` | 1 min | How often the scraper task wakes to select and scrape candidate URLs |
| `spider_batch_size` | `i64` | 10 | Max shops selected per spider tick (`LIMIT` in candidate query) |
| `scraper_batch_size` | `i64` | 100 | Max URLs selected per scraper tick |
| `push_batch_size` | `usize` | 25 | Number of scraped products accumulated before flushing a push to the backend. Keeps memory bounded and avoids holding all results until the last scrape finishes. |
| `spider_concurrency` | `usize` | 3 | Max concurrent spider runs (fan-out via `buffer_unordered`) |
| `scraper_concurrency` | `usize` | 10 | Max concurrent scrape operations |
| `spider_classify_threshold` | `usize` | 200 | Passed through to `SpiderServiceConfig::classify_threshold` — number of URLs buffered before triggering mid-run LLM URL classification |
| `db_max_connections` | `Option<u32>` | `None` (auto) | Maximum Postgres connections in the pool. Each concurrent spider task holds one connection for its `DomainAdvisoryLock` and each scraper task holds one for its `UrlAdvisoryLock` for the full duration of the run. When `None`, auto-computed as `spider_concurrency + scraper_concurrency + 10`. |

---

## `SpiderServiceConfig`

Controls per-run behavior of the spider.

| Field | Type | Default | Description |
|---|---|---|---|
| `classify_threshold` | `usize` | 200 | Number of URLs buffered before triggering mid-run LLM URL classification. Populated from `CrawlerCronConfig::spider_classify_threshold`. |
| `db_batch_size` | `usize` | 100 | Number of `shop_urls` rows flushed per UNNEST batch upsert |
| `max_sample_urls` | `usize` | 500 | Max URLs sent to the URL classification LLM |
| `lock_timeout` | `Duration` | 30 min | How long a `shop_domains.locked_at` lock is considered valid before it can be overridden (spider-service-level optimistic lock) |
| `bloom_capacity` | `usize` | 100_000 | Bloom filter capacity (max unique URLs tracked for dedup per crawl run) |
| `bloom_fp_rate` | `f64` | 0.001 | Bloom filter false-positive rate |

---

## Interactions Between Settings

- **`shop_sync_interval`**: The sync runs once at startup regardless of this value, so a newly deployed instance always has an up-to-date shop list before the spider's first tick. Set this shorter than `spider_interval` if you need faster shop discovery; the default 3-hour cadence is sufficient for typical onboarding flows.

- **`classify_threshold` vs `max_sample_urls`**: Classification is triggered at `classify_threshold` URLs buffered, but only up to `max_sample_urls` are sent to the LLM. If more URLs arrive before the threshold is hit, they are still buffered (for DB persistence) but only the first `max_sample_urls` are included in the LLM prompt. `spider_classify_threshold` in `CrawlerCronConfig` is passed directly to `SpiderServiceConfig::classify_threshold` at construction time.

- **`spider_batch_size` + `spider_concurrency`**: At each tick, up to `spider_batch_size` shops are selected and up to `spider_concurrency` are crawled concurrently. If a domain's advisory lock (`DomainAdvisoryLock`) is held by another worker the item is skipped immediately (counted as `skipped` in the batch log) rather than failing.

- **`lock_timeout`**: Prevents a crashed spider run from blocking a shop indefinitely via the `shop_domains.locked_at` optimistic lock. If `locked_at` is older than `lock_timeout`, the lock is treated as expired and can be acquired by the next run. The cron-level advisory lock (`DomainAdvisoryLock`) is self-healing at the Postgres session level — it is released automatically if the process dies.

- **`db_max_connections`**: Each concurrent spider/scraper task holds one dedicated Postgres connection open for its advisory lock for the full duration of the run. The auto-computed value (`spider_concurrency + scraper_concurrency + 10`) ensures there is always headroom for the short-lived query connections issued by repositories and the shop-sync task. Override with an explicit value if running additional connection consumers.

- **`push_batch_size`**: Controls how often the scraper loop flushes scraped products to the backend push service. Smaller values reduce peak memory at the cost of more push calls; larger values amortise push overhead but hold more results in memory.

- **`db_batch_size`**: Tune this based on Postgres UNNEST performance. Larger batches reduce round-trips but increase per-statement memory. Default of 100 is conservative.
