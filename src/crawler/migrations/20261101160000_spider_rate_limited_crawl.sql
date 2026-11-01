-- A crawl the spider stopped on a shop's rate-limit answer is recorded as its own outcome. The
-- domain keeps its rate-limit streak, which slows its next crawls, and the earliest time the shop
-- allows its next crawl, which no other crawl-timer write, such as a review decision releasing the
-- domain, can bring forward (docs/crawler/scheduling-and-retries.md, rate-limited crawls).
ALTER TABLE listing_source_domains
    ADD COLUMN crawl_rate_limit_streak INT NOT NULL DEFAULT 0,
    ADD COLUMN crawl_rate_limited_until TIMESTAMPTZ;

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
