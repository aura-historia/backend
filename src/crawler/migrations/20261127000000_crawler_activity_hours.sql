-- What the scraper and the spider did in each UTC clock hour, counted as the work ends: one row per
-- hour with one column per count, so a write is one upsert and the Overview reads 24 rows. The
-- contract is in `docs/crawler/operations.md#activity-history`.
CREATE TABLE crawler_activity_hours (
    hour TIMESTAMPTZ PRIMARY KEY CHECK (EXTRACT(EPOCH FROM hour)::BIGINT % 3600 = 0),
    pages_scraped BIGINT NOT NULL DEFAULT 0 CHECK (pages_scraped >= 0),
    page_scrapes_failed BIGINT NOT NULL DEFAULT 0 CHECK (page_scrapes_failed >= 0),
    crawl_runs_completed BIGINT NOT NULL DEFAULT 0 CHECK (crawl_runs_completed >= 0),
    crawl_runs_failed BIGINT NOT NULL DEFAULT 0 CHECK (crawl_runs_failed >= 0),
    crawl_runs_other BIGINT NOT NULL DEFAULT 0 CHECK (crawl_runs_other >= 0),
    product_urls_found BIGINT NOT NULL DEFAULT 0 CHECK (product_urls_found >= 0)
);

-- The hour the counting began, so the console can say since when it records.
INSERT INTO crawler_activity_hours (hour) VALUES (date_trunc('hour', NOW(), 'UTC'));
