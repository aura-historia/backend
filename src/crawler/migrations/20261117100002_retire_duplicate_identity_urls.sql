-- Product URLs of one ListingSource captured under the same source listing id: spellings of one
-- product whose first scrapes ran in one batch before the batch claimed the identities it captured
-- (docs/crawler/networking.md, duplicate source listing ids), so both kept capturing one listing.
-- The earliest stored row keeps capturing it, as the rule keeps the first stored row; the others
-- are retired as DUPLICATE as their first scrape would be now, with the write the scraper makes
-- for a retired duplicate. Re-running it changes nothing more.
WITH ranked AS (
    SELECT url,
           row_number() OVER (
               PARTITION BY listing_source_id, last_source_listing_id
               ORDER BY created, url
           ) AS position
    FROM listing_source_urls
    WHERE url_class = 'product'
      AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')
      AND last_source_listing_id IS NOT NULL
)
UPDATE listing_source_urls AS urls
SET crawler_disposition = 'DUPLICATE',
    next_retry_at = NULL,
    updated = NOW()
FROM ranked
WHERE urls.url = ranked.url
  AND ranked.position > 1;
