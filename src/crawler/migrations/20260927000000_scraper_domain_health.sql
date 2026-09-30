ALTER TABLE listing_source_domains
    ADD COLUMN scrape_failure_streak INT NOT NULL DEFAULT 0,
    ADD COLUMN last_scrape_error_kind TEXT,
    ADD COLUMN last_scrape_status_code INT,
    ADD COLUMN next_scrape_at TIMESTAMPTZ;

ALTER TABLE listing_source_domains
    ADD CONSTRAINT listing_source_domains_scrape_failure_streak_check
        CHECK (scrape_failure_streak >= 0);

CREATE INDEX idx_listing_source_domains_next_scrape_at
    ON listing_source_domains (next_scrape_at)
    WHERE next_scrape_at IS NOT NULL;
