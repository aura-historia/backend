-- The redirect target whose sharing captured the URL's last removal: a page outside the product
-- URL pattern that many product URLs of the ListingSource redirect to (docs/crawler/networking.md,
-- redirect removal evidence). NULL for any other last capture, like `removed_by_templates`, so the
-- evidence survives the removal write and later failures until the next capture replaces it.
ALTER TABLE listing_source_urls
    ADD COLUMN removed_by_redirect_target TEXT,
    ADD CONSTRAINT listing_source_urls_removed_by_redirect_target_nonempty
        CHECK (removed_by_redirect_target IS NULL OR char_length(removed_by_redirect_target) > 0);

-- The scraper counts a ListingSource's product URLs that stand at one redirect target.
CREATE INDEX idx_listing_source_urls_unsettled_redirect_target
    ON listing_source_urls (listing_source_id, unsettled_redirect_target)
    WHERE unsettled_redirect_target IS NOT NULL;
CREATE INDEX idx_listing_source_urls_removed_by_redirect_target
    ON listing_source_urls (listing_source_id, removed_by_redirect_target)
    WHERE removed_by_redirect_target IS NOT NULL;
