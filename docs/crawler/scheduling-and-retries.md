# Crawler scheduling and retries

## Work admission and scheduling

Spider and scraper loops use global bounded schedulers. Before either begins work, it refreshes the complete authoritative ListingSource scope. If that refresh fails, no candidates are selected for that pass.

Spider work is selected by configured domain. Scraper work is selected by eligible domain and URL: a pass avoids selecting the same domain twice, and the production server takes up to 100 due URLs per selected domain. Local process locks prevent duplicate work in one process. Durable crawler state supplies cooldowns and failure metadata across processes and later runs.

The production server currently schedules spider work every 72 hours and scraper work every 10 minutes. These are runtime composition settings, not public timing guarantees.

## Fetch retries

A scraper request receives up to three inline attempts. Backoff begins at one second and is capped at two seconds. `Retry-After` never blocks a domain worker.

After final failure, the crawler records retry metadata and a future `next_retry_at`; it does not hold a worker open. HTTP 404 and 410 are terminal removals. Unsafe targets and other terminal outcomes are not retried immediately. Retryable transport and selected HTTP failures receive a durable cooldown.

Spider failures use the same durable principle. Repeated failures of the same kind increase the domain cooldown. Access blocks, JavaScript-only sites, and similar durable blockers receive longer cooling than transient connection, rate-limit, or server failures.

## Scrape completion fences

The crawler marks a URL scraped only after raw capture reports a terminal persisted outcome (`Changed`, `Unchanged`, `Duplicate`, or `Stale`). Crawler-local completion writes fence on the raw-input SHA-256 observed by that candidate: a later capture makes an older completion or error write a no-op.

A capture rejected because its ListingSource no longer exists is terminal for that queued item but is not treated as persistence. The next authoritative scope refresh disables the stale local work. Other capture failures stay retryable.

`ACTIVE` and `DORMANT_SOLD` URLs remain scrape candidates. Only a durably captured, resolved sold-out result may enter `DORMANT_SOLD`; uncertain, unsupported, invalid, and transient outcomes remain active. A verified removal captures a raw delete, then returns the URL to `ACTIVE` for future checks.

## Throughput and ordering

Scraper workers hand observations to one bounded collector per scheduler pass. Producers wait for capacity. The collector flushes a partial batch when it reaches its size limit, its maximum age, or channel close, and never overlaps flushes.

Within a `(ListingSourceId, candidate URL)` raw stream, capture order is preserved. Independent streams may run in parallel. Local crawler state and business raw capture use separate Postgres transactions, so a committed capture followed by a failed local mark is safely retried through the unchanged path.
