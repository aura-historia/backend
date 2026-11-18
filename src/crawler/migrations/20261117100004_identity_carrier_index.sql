-- The scraper looks up the carrier of an identity (docs/crawler/scheduling-and-retries.md, captured
-- identity) on every first scrape and on every scrape of a captured URL whose page shows another
-- identity, which after a change of a shop's identity rule is each of its URLs. Without this
-- index each lookup reads every URL of the ListingSource.
CREATE INDEX idx_listing_source_urls_carried_identity
    ON listing_source_urls (listing_source_id, last_source_listing_id)
    WHERE last_source_listing_id IS NOT NULL;
