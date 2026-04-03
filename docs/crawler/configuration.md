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
| `spider_batch_size` | `usize` | 10 | Max shops selected per spider tick (`LIMIT` in candidate query) |
| `scraper_batch_size` | `usize` | 100 | Max URLs selected per scraper tick |
| `spider_concurrency` | `usize` | 3 | Max concurrent spider runs (fan-out via `buffer_unordered`) |
| `scraper_concurrency` | `usize` | 10 | Max concurrent scrape operations |

---

## `SpiderServiceConfig`

Controls per-run behavior of the spider.

| Field | Type | Default | Description |
|---|---|---|---|
| `classify_threshold` | `usize` | 200 | Number of URLs buffered before triggering mid-run LLM URL classification |
| `db_batch_size` | `usize` | 100 | Number of `shop_urls` rows flushed per UNNEST batch upsert |
| `max_sample_urls` | `usize` | 500 | Max URLs sent to the URL classification LLM |
| `lock_timeout` | `Duration` | 30 min | How long a `shop_domains.locked_at` lock is considered valid before it can be overridden |
| `bloom_capacity` | `usize` | 100_000 | Bloom filter capacity (max unique URLs tracked for dedup per crawl run) |
| `bloom_fp_rate` | `f64` | 0.001 | Bloom filter false-positive rate |

---

## Interactions Between Settings

- **`shop_sync_interval`**: The sync runs once at startup regardless of this value, so a newly deployed instance always has an up-to-date shop list before the spider's first tick. Set this shorter than `spider_interval` if you need faster shop discovery; the default 3-hour cadence is sufficient for typical onboarding flows.

- **`classify_threshold` vs `max_sample_urls`**: Classification is triggered at `classify_threshold` URLs buffered, but only up to `max_sample_urls` are sent to the LLM. If more URLs arrive before the threshold is hit, they are still buffered (for DB persistence) but only the first `max_sample_urls` are included in the LLM prompt.

- **`spider_batch_size` + `spider_concurrency`**: At each tick, up to `spider_batch_size` shops are selected and up to `spider_concurrency` are crawled concurrently. Shops that fail to acquire the optimistic lock return early without consuming a concurrency slot for long.

- **`lock_timeout`**: Prevents a crashed spider run from blocking a shop indefinitely. If `locked_at` is older than `lock_timeout`, the lock is treated as expired and can be acquired by the next run.

- **`db_batch_size`**: Tune this based on Postgres UNNEST performance. Larger batches reduce round-trips but increase per-statement memory. Default of 100 is conservative.
