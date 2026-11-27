-- When the spider last covered a ListingSource completely: a full crawl that ran out of links to
-- follow, neither cut short by its time budget nor bounded by its page budget. `last_crawled`
-- records every full crawl that reached its bounds, the page budget included, and schedules the
-- next one; this column only answers how many sources were crawled completely
-- (docs/crawler/operations.md, review service).
ALTER TABLE listing_sources ADD COLUMN last_complete_crawl_at TIMESTAMPTZ;

-- The crawler did not record the page-budget case apart before. A source whose latest run
-- completed counts its last full crawl as complete; one whose latest run hit the page budget, or
-- that a later incremental run hides, waits for its next full crawl, at most a week away.
UPDATE listing_sources
SET last_complete_crawl_at = last_crawled
WHERE last_crawled IS NOT NULL
  AND last_crawl_outcome = 'COMPLETED';
