-- A review page the author marked as no product page of a product-schema review, with the reason it
-- gave: NULL for a page the candidate has to resolve. A marked page is left out of the schema
-- matrix and of the review's coverage, and approving the review retires its URL as a product URL of
-- the ListingSource (docs/crawler/extraction-and-review.md, review pages).
ALTER TABLE crawler_review_pages
    ADD COLUMN not_product_reason TEXT
        CONSTRAINT crawler_review_pages_not_product_reason_check
            CHECK (not_product_reason IS NULL OR btrim(not_product_reason) <> '');

-- The trail records marking a page and clearing the mark as changes of the review.
ALTER TABLE crawler_review_events DROP CONSTRAINT crawler_review_events_action_check;
ALTER TABLE crawler_review_events ADD CONSTRAINT crawler_review_events_action_check CHECK (
    action IN (
        'CANDIDATE_EDITED',
        'SCHEMA_FIELD_EDITED',
        'PAGE_ADDED',
        'PAGE_REMOVED',
        'PAGE_MARKED_NOT_PRODUCT',
        'PAGE_NOT_PRODUCT_CLEARED',
        'NO_SOLD_PAGE_RECORDED',
        'REPAIR_REQUESTED',
        'HANDED_TO_HUMAN',
        'APPROVED',
        'REJECTED',
        'RESTORED',
        'REJECTION_REVOKED'
    )
);
