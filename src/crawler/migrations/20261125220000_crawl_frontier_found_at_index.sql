-- The spider resumes a crawl from the oldest frontier URLs first (`ORDER BY found_at, url`),
-- which sorted the ListingSource's whole frontier on every crawl.
CREATE INDEX idx_listing_source_crawl_frontier_found_at
    ON listing_source_crawl_frontier (listing_source_id, found_at, url);
