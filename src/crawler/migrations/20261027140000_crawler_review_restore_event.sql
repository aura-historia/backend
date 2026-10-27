-- A human's restore of what a review's approval replaced is recorded as its own event.
ALTER TABLE crawler_review_events
    DROP CONSTRAINT crawler_review_events_action_check;
ALTER TABLE crawler_review_events
    ADD CONSTRAINT crawler_review_events_action_check CHECK (
        action IN (
            'CANDIDATE_EDITED',
            'SCHEMA_FIELD_EDITED',
            'PAGE_ADDED',
            'PAGE_REMOVED',
            'REPAIR_REQUESTED',
            'APPROVED',
            'REJECTED',
            'RESTORED'
        )
    );

-- The review agent's approvals and rejections of the last hour, which its hourly cap counts.
CREATE INDEX idx_crawler_review_events_agent_decisions
    ON crawler_review_events (created)
    WHERE actor = 'AGENT' AND action IN ('APPROVED', 'REJECTED');
