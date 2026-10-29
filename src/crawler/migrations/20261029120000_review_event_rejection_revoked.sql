-- A human's revocation of a rejected manual product-schema review joins the review's event trail,
-- so the review stays rejected with its history while the rejection stops classifying its pages
-- (docs/crawler/operations.md, manual authoring).
ALTER TABLE crawler_review_events DROP CONSTRAINT crawler_review_events_action_check;

ALTER TABLE crawler_review_events ADD CONSTRAINT crawler_review_events_action_check CHECK (
    action IN (
        'CANDIDATE_EDITED',
        'SCHEMA_FIELD_EDITED',
        'PAGE_ADDED',
        'PAGE_REMOVED',
        'REPAIR_REQUESTED',
        'APPROVED',
        'REJECTED',
        'RESTORED',
        'REJECTION_REVOKED'
    )
);
