-- A page that waits on a pending product-schema review remembers which review it waited on
-- and how many consecutive scrapes waited on it, so its retry delay can grow while the same
-- review stays pending. Both are meaningful only while `last_error_kind` is
-- `PendingSchemaReview`.
ALTER TABLE listing_source_urls
    ADD COLUMN schema_review_waited_on   UUID,
    ADD COLUMN schema_review_wait_streak INT NOT NULL DEFAULT 0,
    ADD CONSTRAINT listing_source_urls_schema_review_wait_shape_check CHECK (
        (schema_review_waited_on IS NULL AND schema_review_wait_streak = 0)
        OR (schema_review_waited_on IS NOT NULL AND schema_review_wait_streak > 0)
    );
