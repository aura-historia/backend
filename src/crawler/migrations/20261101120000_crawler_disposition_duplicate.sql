-- A crawler URL whose product page the crawler stores under another URL, its canonical URL or
-- the known destination it redirects to, is retired as `DUPLICATE` and never scraped again.
ALTER TABLE listing_source_urls
    DROP CONSTRAINT listing_source_urls_crawler_disposition_check;

ALTER TABLE listing_source_urls
    ADD CONSTRAINT listing_source_urls_crawler_disposition_check
        CHECK (crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD', 'DUPLICATE'));
