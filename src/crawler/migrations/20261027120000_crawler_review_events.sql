-- The append-only trail of who changed each review and how. Every accepted review-console
-- mutation appends one row in its own transaction, and a refused mutation none. An event is
-- never updated, and deleted only with its review.
CREATE TABLE crawler_review_events (
    event_id          UUID        PRIMARY KEY,
    review_id         UUID        NOT NULL REFERENCES crawler_reviews(review_id) ON DELETE CASCADE,
    actor             TEXT        NOT NULL CHECK (actor IN ('HUMAN', 'AGENT', 'SYSTEM')),
    action            TEXT        NOT NULL CHECK (
        action IN (
            'CANDIDATE_EDITED',
            'SCHEMA_FIELD_EDITED',
            'PAGE_ADDED',
            'PAGE_REMOVED',
            'REPAIR_REQUESTED',
            'APPROVED',
            'REJECTED'
        )
    ),
    -- The review's candidate version after the mutation.
    candidate_version BIGINT      NOT NULL,
    notes             TEXT,
    details           JSONB       NOT NULL CHECK (jsonb_typeof(details) = 'object'),
    -- The time of the write, not of the transaction's start: a review's mutations append under
    -- its row lock, so this orders a review's events as they committed.
    created           TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);

CREATE INDEX idx_crawler_review_events_review
    ON crawler_review_events (review_id, created);

-- Refuses every update and every delete except the cascade of the review's own deletion, which
-- runs from the foreign key's trigger and so one trigger level deeper than a direct statement.
CREATE FUNCTION refuse_crawler_review_event_change()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF TG_OP = 'DELETE' AND pg_trigger_depth() > 1 THEN
        RETURN OLD;
    END IF;
    RAISE EXCEPTION 'crawler_review_events is append-only';
END;
$$;

CREATE TRIGGER crawler_review_events_append_only
    BEFORE UPDATE OR DELETE ON crawler_review_events
    FOR EACH ROW
    EXECUTE FUNCTION refuse_crawler_review_event_change();
