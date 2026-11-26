-- The scraper chooses its candidates several times per pass and counts the due URLs once per
-- pass (`src/crawler/src/scraper/candidate_service.rs`). A product URL is due once its last
-- scrape, `-infinity` when it was never scraped, is a day old.
--
-- The due URLs of one ListingSource are one range of this index, in the order the scraper takes
-- them, so choosing candidates reads a few sources' first URLs instead of every due URL.
CREATE INDEX idx_listing_source_urls_scrape_due
    ON listing_source_urls (listing_source_id, COALESCE(last_scraped, '-infinity'::timestamptz), url)
    WHERE url_class = 'product' AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD');

-- Counting every due URL reads them in table order from this one. It replaces the index on
-- (url_class, last_scraped), which only the due-URL query used and which cannot serve the
-- `COALESCE` spelling of the due condition.
CREATE INDEX idx_listing_source_urls_scrape_due_since
    ON listing_source_urls (COALESCE(last_scraped, '-infinity'::timestamptz))
    WHERE url_class = 'product' AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD');
DROP INDEX idx_listing_source_urls_class_last_scraped;
