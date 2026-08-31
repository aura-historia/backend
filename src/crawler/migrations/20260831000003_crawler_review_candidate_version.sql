ALTER TABLE crawler_reviews
    ADD COLUMN IF NOT EXISTS candidate_version BIGINT NOT NULL DEFAULT 1;

CREATE OR REPLACE FUNCTION increment_crawler_review_candidate_version()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.candidate_payload IS DISTINCT FROM OLD.candidate_payload THEN
        NEW.candidate_version = OLD.candidate_version + 1;
    END IF;
    RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS crawler_reviews_candidate_version ON crawler_reviews;

CREATE TRIGGER crawler_reviews_candidate_version
    BEFORE UPDATE OF candidate_payload ON crawler_reviews
    FOR EACH ROW
    EXECUTE FUNCTION increment_crawler_review_candidate_version();
