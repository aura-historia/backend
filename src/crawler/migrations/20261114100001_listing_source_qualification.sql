-- Whether a ListingSource is a real antique e-shop, decided before it is crawled
-- (docs/crawler/README.md, ListingSource qualification). Nothing existing changes: every source
-- the scope admits now joins the one-off backfill and keeps being crawled as before until it is
-- decided.
ALTER TABLE listing_sources
    -- Whether the latest scope refresh admits the source to crawling with a crawl root, before a
    -- label or a missing qualification keeps it disabled: the sources a qualification is for.
    ADD COLUMN in_crawl_scope BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN qualification TEXT
        CHECK (qualification IS NULL
            OR qualification IN ('QUALIFIED', 'DISQUALIFIED', 'NEEDS_HUMAN')),
    ADD COLUMN qualification_note TEXT
        CHECK (qualification_note IS NULL
            OR (btrim(qualification_note) <> '' AND char_length(qualification_note) <= 500)),
    ADD COLUMN qualified_at TIMESTAMPTZ,
    ADD COLUMN qualified_by TEXT
        CHECK (qualified_by IS NULL
            OR qualified_by IN ('HUMAN', 'AGENT_AUTHOR', 'AGENT_REVIEWER', 'SYSTEM')),
    -- The source existed before qualification did: it is crawled while it waits for its first
    -- qualification, and that one decision may replace a label a human set.
    ADD COLUMN qualification_backfill BOOLEAN NOT NULL DEFAULT FALSE,
    ADD CONSTRAINT listing_sources_qualification_shape_check
        CHECK ((qualification IS NULL AND qualification_note IS NULL
                AND qualified_at IS NULL AND qualified_by IS NULL)
            OR (qualification IS NOT NULL AND qualification_note IS NOT NULL
                AND qualified_at IS NOT NULL AND qualified_by IS NOT NULL));

-- Only the sources the scope admits now join the backfill; one that is out of scope and returns
-- later waits for its qualification like a new source.
UPDATE listing_sources
SET in_crawl_scope = (crawl_enabled OR source_label IS NOT NULL) AND crawl_root_url IS NOT NULL;
UPDATE listing_sources SET qualification_backfill = in_crawl_scope;

-- One row per qualification decided or cleared, written in the transaction of the change: the
-- decision, the label a disqualification set, the note and who decided. A row is never updated,
-- and deleted only with its source.
CREATE TABLE listing_source_qualification_events (
    event_id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    listing_source_id  UUID        NOT NULL
        REFERENCES listing_sources(listing_source_id) ON DELETE CASCADE,
    qualification      TEXT
        CHECK (qualification IS NULL
            OR qualification IN ('QUALIFIED', 'DISQUALIFIED', 'NEEDS_HUMAN')),
    source_label       TEXT,
    note               TEXT        CHECK (note IS NULL
        OR (btrim(note) <> '' AND char_length(note) <= 500)),
    actor              TEXT        NOT NULL CHECK (
        actor IN ('HUMAN', 'AGENT_AUTHOR', 'AGENT_REVIEWER', 'SYSTEM')),
    occurred_at        TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    -- A cleared qualification (qualification NULL) carries no note; every decision carries one.
    CONSTRAINT listing_source_qualification_events_shape_check CHECK (
        (qualification IS NULL AND note IS NULL AND source_label IS NULL)
        OR (qualification IS NOT NULL AND note IS NOT NULL))
);

CREATE INDEX idx_listing_source_qualification_events_source
    ON listing_source_qualification_events (listing_source_id, occurred_at);

CREATE FUNCTION refuse_listing_source_qualification_event_change()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF TG_OP = 'DELETE' AND pg_trigger_depth() > 1 THEN
        RETURN OLD;
    END IF;
    RAISE EXCEPTION 'listing_source_qualification_events is append-only';
END;
$$;

CREATE TRIGGER listing_source_qualification_events_append_only
    BEFORE UPDATE OR DELETE ON listing_source_qualification_events
    FOR EACH ROW
    EXECUTE FUNCTION refuse_listing_source_qualification_event_change();

-- The sources that wait for a qualification, which the agents' queue reads.
CREATE INDEX idx_listing_sources_awaiting_qualification
    ON listing_sources (listing_source_id)
    WHERE in_crawl_scope AND qualification IS NULL;
