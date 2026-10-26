-- A crawl that stops classifying at a pending URL-pattern review is neither a
-- success nor a failure, and a domain whose deterministic failures keep
-- repeating stops retrying until an operator acts. Both are recorded as their
-- own crawl outcome.
ALTER TABLE listing_source_domains
    DROP CONSTRAINT listing_source_domains_last_crawl_outcome_check,
    ADD CONSTRAINT listing_source_domains_last_crawl_outcome_check
        CHECK (last_crawl_outcome IN (
            'COMPLETED',
            'PAGE_BUDGET_REACHED',
            'HARD_TIMEOUT_TRUNCATED',
            'FAILED',
            'AWAITING_URL_PATTERN_REVIEW',
            'NEEDS_OPERATOR'
        ));
