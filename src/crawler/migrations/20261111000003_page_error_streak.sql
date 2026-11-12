-- The page errors a product URL answered in a row (docs/crawler/scheduling-and-retries.md, fetch
-- retries): every fetch failure that is a page error counts one more, and any other failure or
-- outcome of a scrape resets it. Its retry delay grows with the run, which parks the URL at its
-- threshold. Unlike `failure_count`, it counts no failure of another kind. A constant default
-- adds the column without rewriting the table; the check is added NOT VALID, so it holds for
-- every write without scanning the existing rows, which all carry the default.
ALTER TABLE listing_source_urls
    ADD COLUMN page_error_streak INT NOT NULL DEFAULT 0;

ALTER TABLE listing_source_urls
    ADD CONSTRAINT listing_source_urls_page_error_streak_nonnegative
        CHECK (page_error_streak >= 0) NOT VALID;
