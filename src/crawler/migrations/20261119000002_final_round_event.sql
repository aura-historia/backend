-- The review agents' final round (#2990): the author agent records FINAL_ROUND_STARTED on a
-- pending review that would wait for a human, and the review is the agents' again for one more
-- round with the strongest model before its source is parked (docs/crawler/operations.md, review
-- agents). Every action persisted before stays valid.
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
        'FINAL_ROUND_STARTED',
        'APPROVED',
        'REJECTED',
        'RESTORED',
        'REJECTION_REVOKED'
    )
);
