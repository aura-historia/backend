ALTER TABLE listing_source_domains
    ADD COLUMN last_crawl_outcome TEXT,
    ADD COLUMN crawl_truncation_count INT NOT NULL DEFAULT 0;

ALTER TABLE listing_source_domains
    ADD CONSTRAINT listing_source_domains_last_crawl_outcome_check
        CHECK (last_crawl_outcome IN ('COMPLETED', 'PAGE_BUDGET_REACHED', 'HARD_TIMEOUT_TRUNCATED', 'FAILED')),
    ADD CONSTRAINT listing_source_domains_crawl_truncation_count_check
        CHECK (crawl_truncation_count >= 0);
