-- When each frontier URL was first left unvisited. A crawl seeds the longest-waiting URLs of a
-- frontier larger than its seed limit and carries the rest over, so a large shop's frontier is
-- worked through in the order it was found instead of the same alphabetical window
-- (docs/crawler/scheduling-and-retries.md, discovery coverage). Rows stored before count as
-- found now.
ALTER TABLE listing_source_crawl_frontier
    ADD COLUMN found_at TIMESTAMPTZ NOT NULL DEFAULT now();
