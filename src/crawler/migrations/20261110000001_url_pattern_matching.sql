-- What a ListingSource's live product URL pattern is matched against: FULL_URL_SEARCH, anywhere
-- in the whole URL, or PATH_AND_QUERY_FULL_MATCH, the whole of the URL's path and query
-- (docs/crawler/extraction-and-review.md, URL pattern). Every pattern stored before was matched
-- anywhere in the URL. A matching exists exactly with a pattern.
ALTER TABLE listing_sources
    ADD COLUMN url_pattern_matching TEXT;

UPDATE listing_sources
SET url_pattern_matching = 'FULL_URL_SEARCH'
WHERE url_pattern_state = 'MATCHED';

ALTER TABLE listing_sources
    DROP CONSTRAINT listing_sources_url_pattern_state_shape_check,
    ADD CONSTRAINT listing_sources_url_pattern_state_shape_check CHECK (
        (url_pattern_state = 'MATCHED' AND url_pattern IS NOT NULL
            AND (url_pattern_exclude IS NULL OR btrim(url_pattern_exclude) <> '')
            AND url_pattern_matching IS NOT NULL
            AND url_pattern_matching IN ('FULL_URL_SEARCH', 'PATH_AND_QUERY_FULL_MATCH'))
        OR (url_pattern_state IN ('UNKNOWN', 'NO_PATTERN')
            AND url_pattern IS NULL AND url_pattern_exclude IS NULL
            AND url_pattern_matching IS NULL)
    );
