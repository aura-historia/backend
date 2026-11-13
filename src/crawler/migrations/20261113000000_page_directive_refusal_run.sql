-- The page-directive refusals a product URL answered in a row and when the first of them was
-- recorded (docs/crawler/networking.md, page robots directives): a stored listing whose page
-- keeps refusing capture is withdrawn once the run is long enough. Meaningful only while
-- `last_error_kind` is `PageDirectiveDisallowed`; every other outcome ends the run. Constant
-- defaults add the columns without rewriting the table; the check is added NOT VALID, so it
-- holds for every write without scanning the existing rows, which all carry the defaults.
ALTER TABLE listing_source_urls
    ADD COLUMN page_directive_refusal_streak INT NOT NULL DEFAULT 0,
    ADD COLUMN page_directive_refused_since TIMESTAMPTZ;

ALTER TABLE listing_source_urls
    ADD CONSTRAINT listing_source_urls_page_directive_refusal_run_shape CHECK (
        (page_directive_refusal_streak = 0 AND page_directive_refused_since IS NULL)
        OR (page_directive_refusal_streak > 0 AND page_directive_refused_since IS NOT NULL)
    ) NOT VALID;
