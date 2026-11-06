-- A crawl the crawler's shutdown ended keeps its frontier and is neither a truncation nor a
-- failure, so it is recorded as its own crawl outcome.
ALTER TABLE listing_sources
    DROP CONSTRAINT listing_sources_last_crawl_outcome_check;

ALTER TABLE listing_sources
    ADD CONSTRAINT listing_sources_last_crawl_outcome_check
        CHECK (last_crawl_outcome IN (
            'COMPLETED',
            'PAGE_BUDGET_REACHED',
            'HARD_TIMEOUT_TRUNCATED',
            'STOPPED',
            'RATE_LIMITED',
            'FAILED',
            'AWAITING_URL_PATTERN_REVIEW',
            'NEEDS_OPERATOR'
        ));
