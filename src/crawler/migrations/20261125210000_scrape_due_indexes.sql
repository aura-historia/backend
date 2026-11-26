-- The scraper chooses its candidates several times per pass and counts the due URLs once per
-- pass (`src/crawler/src/scraper/candidate_service.rs`). A product URL is due once its last
-- scrape, `-infinity` when it was never scraped, is a day old.
--
-- Choosing candidates looks up whether each source has a due URL and reads the first due URLs
-- of a few sources. Keyed by source and last scrape, each source's due URLs are one range of
-- this index, so neither grows with how many URLs are due over all sources.
CREATE INDEX idx_listing_source_urls_scrape_due
    ON listing_source_urls (listing_source_id, COALESCE(last_scraped, '-infinity'::timestamptz), url)
    WHERE url_class = 'product' AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD');

-- The count of every due URL across sources needs the due URLs without a source key. This
-- index serves the `COALESCE` spelling of the due condition; the index on
-- (url_class, last_scraped) it replaces served only the earlier spelling and no other query.
CREATE INDEX idx_listing_source_urls_scrape_due_since
    ON listing_source_urls (COALESCE(last_scraped, '-infinity'::timestamptz))
    WHERE url_class = 'product' AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD');
DROP INDEX idx_listing_source_urls_class_last_scraped;
