-- The author and reviewer agents of `agent` authoring mode record their review changes as actors
-- of their own. Events of the single review agent before them keep `AGENT`.
ALTER TABLE crawler_review_events
    DROP CONSTRAINT crawler_review_events_actor_check;
ALTER TABLE crawler_review_events
    ADD CONSTRAINT crawler_review_events_actor_check CHECK (
        actor IN ('HUMAN', 'AGENT', 'AGENT_AUTHOR', 'AGENT_REVIEWER', 'SYSTEM')
    );

-- The review agents' approvals and rejections of the last hour, which their hourly cap counts.
DROP INDEX idx_crawler_review_events_agent_decisions;
CREATE INDEX idx_crawler_review_events_agent_decisions
    ON crawler_review_events (created)
    WHERE actor IN ('AGENT', 'AGENT_AUTHOR', 'AGENT_REVIEWER')
      AND action IN ('APPROVED', 'REJECTED');

-- A candidate change also drops a URL-pattern review's live evaluation of its URL sample, which
-- was made for the decision the change replaces.
CREATE OR REPLACE FUNCTION increment_crawler_review_candidate_version()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.candidate_payload IS DISTINCT FROM OLD.candidate_payload THEN
        NEW.candidate_version = OLD.candidate_version + 1;
        NEW.validation_summary = NEW.validation_summary
            - 'schema_matrix' - 'schema_matrix_summary' - 'url_sample_evaluation';
    END IF;
    RETURN NEW;
END;
$$;
