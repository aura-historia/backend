-- When the review agents try a ListingSource again that they parked (#2990;
-- docs/crawler/README.md, ListingSource scope and qualification). Each time is decided when the
-- park is stored, from the source's label or qualification history, and cleared with it:
--
-- - source_label_retry_at: an agent's label that expires (BOT_PROTECTION, JAVASCRIPT_RENDERED,
--   LOGIN_REQUIRED, the two approach labels, AGENTS_UNRESOLVED) is cleared then and the source
--   goes back to qualification. A human's label never carries one.
-- - qualification_retry_at: an agent's NEEDS_HUMAN qualification is decided again then.
ALTER TABLE listing_sources
    ADD COLUMN source_label_retry_at TIMESTAMPTZ,
    ADD COLUMN qualification_retry_at TIMESTAMPTZ,
    ADD CONSTRAINT listing_sources_source_label_retry_check
        CHECK (source_label_retry_at IS NULL OR (source_label IN (
                'BOT_PROTECTION', 'JAVASCRIPT_RENDERED', 'LOGIN_REQUIRED',
                'URL_PATTERN_APPROACH_NOT_APPLICABLE', 'PRODUCT_SCHEMA_APPROACH_NOT_APPLICABLE',
                'AGENTS_UNRESOLVED')
            AND source_labelled_by IN ('AGENT_AUTHOR', 'AGENT_REVIEWER'))),
    ADD CONSTRAINT listing_sources_qualification_retry_check
        CHECK (qualification_retry_at IS NULL
            OR (qualification = 'NEEDS_HUMAN' AND qualified_by IN ('AGENT_AUTHOR', 'AGENT_REVIEWER')));

-- The agents' labels of these kinds that stand today were set when only a human cleared a label;
-- they expire like a first park, 30 days after they were set, the default first interval.
UPDATE listing_sources
SET source_label_retry_at = source_labelled_at + INTERVAL '30 days'
WHERE source_labelled_by IN ('AGENT_AUTHOR', 'AGENT_REVIEWER')
  AND source_label IN (
      'BOT_PROTECTION', 'JAVASCRIPT_RENDERED', 'LOGIN_REQUIRED',
      'URL_PATTERN_APPROACH_NOT_APPLICABLE', 'PRODUCT_SCHEMA_APPROACH_NOT_APPLICABLE');

-- An agent's NEEDS_HUMAN that stands today is tried again a day after it, as before; one whose
-- daily tries are spent, which waited for a human, gets the agents' final round then.
UPDATE listing_sources
SET qualification_retry_at = qualified_at + INTERVAL '24 hours'
WHERE qualification = 'NEEDS_HUMAN' AND qualified_by IN ('AGENT_AUTHOR', 'AGENT_REVIEWER');

CREATE INDEX idx_listing_sources_source_label_retry
    ON listing_sources (source_label_retry_at)
    WHERE source_label_retry_at IS NOT NULL;
