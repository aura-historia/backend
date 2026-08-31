ALTER TABLE listing_source_domains
    ADD COLUMN IF NOT EXISTS url_pattern_state TEXT NOT NULL DEFAULT 'UNKNOWN';

UPDATE listing_source_domains
SET url_pattern_state = 'MATCHED'
WHERE url_pattern IS NOT NULL
  AND url_pattern_state = 'UNKNOWN';

ALTER TABLE listing_source_domains
    DROP CONSTRAINT IF EXISTS listing_source_domains_url_pattern_state_check,
    DROP CONSTRAINT IF EXISTS listing_source_domains_url_pattern_state_shape_check;

ALTER TABLE listing_source_domains
    ADD CONSTRAINT listing_source_domains_url_pattern_state_check
    CHECK (url_pattern_state IN ('UNKNOWN', 'MATCHED', 'NO_PATTERN'));

ALTER TABLE listing_source_domains
    ADD CONSTRAINT listing_source_domains_url_pattern_state_shape_check
    CHECK (
        (url_pattern_state = 'MATCHED' AND url_pattern IS NOT NULL)
        OR (url_pattern_state IN ('UNKNOWN', 'NO_PATTERN') AND url_pattern IS NULL)
    );
