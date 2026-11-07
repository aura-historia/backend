-- A review agent can hand a pending review to a human at once, which the review's trail records
-- as its own event action with the agent's reasoning as the note.
ALTER TABLE crawler_review_events
    DROP CONSTRAINT crawler_review_events_action_check;

ALTER TABLE crawler_review_events
    ADD CONSTRAINT crawler_review_events_action_check
        CHECK (action IN (
            'CANDIDATE_EDITED',
            'SCHEMA_FIELD_EDITED',
            'PAGE_ADDED',
            'PAGE_REMOVED',
            'REPAIR_REQUESTED',
            'HANDED_TO_HUMAN',
            'APPROVED',
            'REJECTED',
            'RESTORED',
            'REJECTION_REVOKED'
        ));
