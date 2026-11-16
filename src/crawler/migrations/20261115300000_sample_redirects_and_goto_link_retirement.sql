-- Where a URL-pattern review's sample URL led when the console last fetched it to see whether the
-- candidate's URLs reach product pages: the URL itself when it did not redirect. NULL for a URL the
-- console never fetched. The approval guard refuses a pattern most of whose fetched URLs redirect
-- to no product page under it (docs/crawler/extraction-and-review.md, approval guard).
ALTER TABLE crawler_review_urls
    ADD COLUMN redirect_target TEXT,
    ADD COLUMN redirect_checked TIMESTAMPTZ,
    ADD CONSTRAINT crawler_review_urls_redirect_shape_check CHECK (
        (redirect_target IS NULL AND redirect_checked IS NULL)
        OR (char_length(redirect_target) > 0 AND redirect_checked IS NOT NULL)
    );

-- One-off pass for #2986. URL-pattern reviews approved site-builder `goto` links
-- (`/app/module/webproduct/goto/m/<id>`) as product URLs. Each such link redirects to a category
-- page at the product's anchor, so no scrape extracts it and the scraper held the URLs as
-- RedirectTargetUnverified or under the mass-removal guard without end. Nothing was ever captured
-- from them.

-- The sources the pass acts on, before any of their rows change.
CREATE TEMPORARY TABLE goto_link_sources ON COMMIT DROP AS
SELECT DISTINCT u.listing_source_id
FROM listing_source_urls u
WHERE u.url_class = 'product' AND u.url LIKE '%/app/module/webproduct/goto/%';

-- Retires the `goto` links as product URLs, as a rejected page is retired: the row stays, out of
-- the product class, with its retry delay and unsettled redirect cleared.
UPDATE listing_source_urls
SET url_class = 'other', next_retry_at = NULL,
    unsettled_redirect_target = NULL, unsettled_redirect_streak = 0, updated = NOW()
WHERE url_class = 'product' AND url LIKE '%/app/module/webproduct/goto/%'
  AND listing_source_id IN (SELECT listing_source_id FROM goto_link_sources);

-- A live pattern that takes `goto` links would store them as product URLs again on the next crawl,
-- so the source's pattern is forgotten and its next crawl opens a URL-pattern review. The approval
-- guard refuses `goto` links there by where they redirect, and its finding tells the author to
-- write the pattern for the pages they land on when those are product pages, or to label the
-- source URL_PATTERN_APPROACH_NOT_APPLICABLE when the shop has none. The pass labels nothing: on
-- the shop checked by hand the landing page carries one schema.org Product, so the source may
-- well be crawlable.
UPDATE listing_sources
SET url_pattern_state = 'UNKNOWN', url_pattern = NULL, url_pattern_exclude = NULL,
    url_pattern_matching = NULL, updated = NOW()
WHERE listing_source_id IN (SELECT listing_source_id FROM goto_link_sources)
  AND url_pattern_state = 'MATCHED'
  AND url_pattern LIKE '%webproduct/goto%';
