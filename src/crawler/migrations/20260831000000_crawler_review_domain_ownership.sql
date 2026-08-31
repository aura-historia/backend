ALTER TABLE crawler_reviews
    DROP CONSTRAINT IF EXISTS crawler_reviews_domain_id_fkey,
    DROP CONSTRAINT IF EXISTS crawler_reviews_listing_source_domain_fkey,
    DROP CONSTRAINT IF EXISTS crawler_reviews_artifact_domain_shape_check;

ALTER TABLE crawler_reviews
    ADD CONSTRAINT crawler_reviews_listing_source_domain_fkey
    FOREIGN KEY (listing_source_id, domain_id)
    REFERENCES listing_source_domains (listing_source_id, domain_id)
    ON DELETE CASCADE;

ALTER TABLE crawler_reviews
    ADD CONSTRAINT crawler_reviews_artifact_domain_shape_check
    CHECK (
        (artifact_type = 'URL_PATTERN' AND domain_id IS NOT NULL)
        OR (artifact_type = 'PRODUCT_SCHEMA' AND domain_id IS NULL)
    );
