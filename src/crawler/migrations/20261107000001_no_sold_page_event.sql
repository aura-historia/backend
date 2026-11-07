-- The author agent's record that it looked for a sold or unavailable page of the ListingSource and
-- found none (docs/crawler/extraction-and-review.md, approval guard). The record is an event of
-- the review's trail that names the URLs it checked.
ALTER TABLE crawler_review_events DROP CONSTRAINT crawler_review_events_action_check;
ALTER TABLE crawler_review_events ADD CONSTRAINT crawler_review_events_action_check CHECK (
    action IN (
        'CANDIDATE_EDITED',
        'SCHEMA_FIELD_EDITED',
        'PAGE_ADDED',
        'PAGE_REMOVED',
        'NO_SOLD_PAGE_RECORDED',
        'REPAIR_REQUESTED',
        'HANDED_TO_HUMAN',
        'APPROVED',
        'REJECTED',
        'RESTORED',
        'REJECTION_REVOKED'
    )
);
