-- The review console's ListingSource overview and health counts read the product URLs that
-- wait on a pending product-schema review on every refresh. Without this index each read scans
-- every URL of every ListingSource, which grows with each crawl.
CREATE INDEX idx_listing_source_urls_pending_schema_review
    ON listing_source_urls (listing_source_id, schema_review_waited_on)
    WHERE last_error_kind = 'PendingSchemaReview';
