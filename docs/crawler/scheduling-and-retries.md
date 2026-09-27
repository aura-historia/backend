# Crawler scheduling and retries

## Work admission and scheduling

Spider and scraper loops use global bounded schedulers. Before either begins work, it refreshes the complete authoritative ListingSource scope. If that refresh fails, no candidates are selected for that pass.

Spider work is selected by configured domain. Scraper work is selected by eligible domain and URL: a pass avoids selecting the same domain twice, and the production server takes up to 100 due URLs per selected domain. Local process locks prevent duplicate work in one process. Durable crawler state supplies cooldowns and failure metadata across processes and later runs.

The production server currently schedules spider work every 72 hours and scraper work every 10 minutes. These are runtime composition settings, not public timing guarantees.

## Fetch retries

A scraper request receives up to three inline attempts. Backoff begins at one second and is capped at two seconds. `Retry-After` never blocks a domain worker.

After final failure, the crawler records retry metadata and a future `next_retry_at`; it does not hold a worker open. HTTP 404 and 410 are terminal removals. Unsafe targets and other terminal outcomes are not retried immediately. Retryable transport and selected HTTP failures receive a durable cooldown.

## Scraper domain circuits

URL retry state and domain health are separate. Timeouts, connection failures, transient DNS failures, HTTP 408, 429, 503, and 504 open the `listing_source_domains` scraper circuit. The domain stores a monotonic `scrape_failure_streak`, stable `last_scrape_error_kind`/status diagnostics, and `next_scrape_at`; cooldowns grow exponentially from the failure kind's base and are capped at 24 hours. A `Retry-After` value is retained as structured fetch metadata and can extend the durable domain cooldown without blocking the worker.

An open domain is excluded before candidate selection. When its cooldown expires, selection returns exactly one probe URL and the scraper uses `DomainProbe` mode, which suppresses schema-seed fan-out. A responsive HTTP result, including 404/410 or a schema/normalization result after a successful fetch, closes the circuit on the next fenced state update. A probe transport failure increments the same streak and reopens the circuit. A single 500/502 remains URL-scoped; three distinct 500/502 failures in one domain batch open the short `HTTP_5XX_BURST` circuit and stop the remaining URLs for that domain.

Opening and closing updates are fenced by the candidate's domain snapshot. The URL failure and circuit opening share one transaction, so stale URL work cannot change current domain health. A local `DomainLock` coordinates scraper and spider work for the same crawler domain, while unrelated domains continue independently.

Spider failures use the same durable principle. Repeated failures of the same kind increase the domain cooldown. Access blocks, JavaScript-only sites, and similar durable blockers receive longer cooling than transient connection, rate-limit, or server failures.

## Scrape completion fences

The crawler marks a URL scraped only after raw capture reports a terminal persisted outcome (`Changed`, `Unchanged`, `Duplicate`, or `Stale`). Crawler-local completion writes fence on the raw-input SHA-256 observed by that candidate: a later capture makes an older completion or error write a no-op.

A capture rejected because its ListingSource no longer exists is terminal for that queued item but is not treated as persistence. The next authoritative scope refresh disables the stale local work. Other capture failures stay retryable.

`ACTIVE` and `DORMANT_SOLD` URLs remain scrape candidates. Only a durably captured, resolved sold-out result may enter `DORMANT_SOLD`; uncertain, unsupported, invalid, and transient outcomes remain active. A verified removal captures a raw delete, then returns the URL to `ACTIVE` for future checks.

## Throughput and ordering

Scraper workers hand observations to one bounded collector per scheduler pass. Producers wait for capacity. The collector flushes a partial batch when it reaches its size limit, its maximum age, or channel close, and never overlaps flushes.

Within a `(ListingSourceId, candidate URL)` raw stream, capture order is preserved. Independent streams may run in parallel. Local crawler state and business raw capture use separate Postgres transactions, so a committed capture followed by a failed local mark is safely retried through the unchanged path.
