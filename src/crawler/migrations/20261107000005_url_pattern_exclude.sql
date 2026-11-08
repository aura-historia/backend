-- The exclusion of a ListingSource's live product URL pattern: a regex for the non-items (gift
-- cards, vouchers, services) that share the product URL shape. A URL is a product URL when it
-- matches url_pattern and does not match url_pattern_exclude
-- (docs/crawler/extraction-and-review.md, URL pattern). An exclusion exists only with a pattern.
ALTER TABLE listing_sources
    ADD COLUMN url_pattern_exclude TEXT,
    DROP CONSTRAINT listing_sources_url_pattern_state_shape_check,
    ADD CONSTRAINT listing_sources_url_pattern_state_shape_check CHECK (
        (url_pattern_state = 'MATCHED' AND url_pattern IS NOT NULL
            AND (url_pattern_exclude IS NULL OR btrim(url_pattern_exclude) <> ''))
        OR (url_pattern_state IN ('UNKNOWN', 'NO_PATTERN')
            AND url_pattern IS NULL AND url_pattern_exclude IS NULL)
    );
