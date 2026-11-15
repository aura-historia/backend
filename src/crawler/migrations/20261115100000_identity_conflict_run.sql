-- The identity conflicts (`SourceListingIdConflict`) a product URL's scrapes recorded in a row,
-- when the first and the latest of them were recorded (docs/crawler/scheduling-and-retries.md,
-- identity conflicts): the streak grows the URL's retry delay, the start tells an operator since
-- when the URL is stuck, and a product-schema change after the latest conflict releases the URL
-- at once. Meaningful only while `last_error_kind` is `SourceListingIdConflict`; every other
-- failure ends the run. Constant defaults add the columns without rewriting the table; the check
-- is added NOT VALID, so it holds for every write without scanning the existing rows.
ALTER TABLE listing_source_urls
    ADD COLUMN identity_conflict_streak INT NOT NULL DEFAULT 0,
    ADD COLUMN identity_conflict_since TIMESTAMPTZ,
    ADD COLUMN identity_conflict_last TIMESTAMPTZ;

ALTER TABLE listing_source_urls
    ADD CONSTRAINT listing_source_urls_identity_conflict_run_shape CHECK (
        (identity_conflict_streak = 0
         AND identity_conflict_since IS NULL
         AND identity_conflict_last IS NULL)
        OR (identity_conflict_streak > 0
            AND identity_conflict_since IS NOT NULL
            AND identity_conflict_last >= identity_conflict_since)
    ) NOT VALID;

-- URLs already in conflict continue their run: each failed write counted one failure, so the
-- failure count stands in for the streak, the last completed scrape for the run's start and the
-- last write for the latest conflict.
UPDATE listing_source_urls
SET identity_conflict_streak = GREATEST(failure_count, 1),
    identity_conflict_since = LEAST(COALESCE(last_scraped, updated), updated),
    identity_conflict_last = updated
WHERE last_error_kind = 'SourceListingIdConflict';
