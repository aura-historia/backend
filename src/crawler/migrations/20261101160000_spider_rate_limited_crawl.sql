-- A crawl the spider stopped on a shop's rate-limit answer is recorded as its own outcome, and the
-- domain keeps how many consecutive crawls were rate-limited since its last full crawl, which
-- slows its next crawls (docs/crawler/scheduling-and-retries.md, rate-limited crawls).
ALTER TABLE listing_source_domains
    ADD COLUMN crawl_rate_limit_streak INT NOT NULL DEFAULT 0;

ALTER TABLE listing_source_domains
    ADD CONSTRAINT listing_source_domains_crawl_rate_limit_streak_check
        CHECK (crawl_rate_limit_streak >= 0);

ALTER TABLE listing_source_domains DROP CONSTRAINT listing_source_domains_last_crawl_outcome_check;

ALTER TABLE listing_source_domains ADD CONSTRAINT listing_source_domains_last_crawl_outcome_check
    CHECK (last_crawl_outcome IN (
        'COMPLETED',
        'PAGE_BUDGET_REACHED',
        'HARD_TIMEOUT_TRUNCATED',
        'RATE_LIMITED',
        'FAILED',
        'AWAITING_URL_PATTERN_REVIEW',
        'NEEDS_OPERATOR'
    ));
