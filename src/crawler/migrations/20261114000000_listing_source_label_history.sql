-- The labels of #2972 and the append-only history of every label a ListingSource is given or
-- loses (docs/crawler/README.md, ListingSource scope). Nothing existing changes: every label
-- persisted before stays valid, APPROACH_NOT_APPLICABLE included, which no new label may use.
ALTER TABLE listing_sources
    DROP CONSTRAINT listing_sources_source_label_check,
    ADD CONSTRAINT listing_sources_source_label_check
        CHECK (source_label IS NULL OR source_label IN (
            'NOT_AN_ESHOP', 'NOT_AN_ANTIQUE_SHOP',
            'URL_PATTERN_APPROACH_NOT_APPLICABLE', 'PRODUCT_SCHEMA_APPROACH_NOT_APPLICABLE',
            'ROBOTS_DISALLOWED', 'SHOP_CLOSED', 'BOT_PROTECTION', 'JAVASCRIPT_RENDERED',
            'LOGIN_REQUIRED', 'OTHER', 'APPROACH_NOT_APPLICABLE'));

-- One row per label set on a source (LABELLED, which replaces any label it had) or cleared from
-- it (CLEARED), written in the transaction of the change. The label a row replaced or cleared is
-- the source's row before it; a row is never updated, and deleted only with its source.
CREATE TABLE listing_source_label_events (
    event_id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    listing_source_id  UUID        NOT NULL
        REFERENCES listing_sources(listing_source_id) ON DELETE CASCADE,
    event_kind         TEXT        NOT NULL CHECK (event_kind IN ('LABELLED', 'CLEARED')),
    source_label       TEXT        CHECK (source_label IS NULL OR source_label IN (
        'NOT_AN_ESHOP', 'NOT_AN_ANTIQUE_SHOP',
        'URL_PATTERN_APPROACH_NOT_APPLICABLE', 'PRODUCT_SCHEMA_APPROACH_NOT_APPLICABLE',
        'ROBOTS_DISALLOWED', 'SHOP_CLOSED', 'BOT_PROTECTION', 'JAVASCRIPT_RENDERED',
        'LOGIN_REQUIRED', 'OTHER', 'APPROACH_NOT_APPLICABLE')),
    source_label_note  TEXT        CHECK (source_label_note IS NULL
        OR (btrim(source_label_note) <> '' AND char_length(source_label_note) <= 500)),
    actor              TEXT        NOT NULL CHECK (
        actor IN ('HUMAN', 'AGENT_AUTHOR', 'AGENT_REVIEWER', 'SYSTEM')),
    -- The time of the write, so a source's events order as they committed under its row lock.
    occurred_at        TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    CONSTRAINT listing_source_label_events_shape_check CHECK (
        (event_kind = 'LABELLED' AND source_label IS NOT NULL)
        OR (event_kind = 'CLEARED' AND source_label IS NULL AND source_label_note IS NULL))
);

CREATE INDEX idx_listing_source_label_events_source
    ON listing_source_label_events (listing_source_id, occurred_at);

-- Refuses every update and every delete except the cascade of the source's own deletion, which
-- runs from the foreign key's trigger one level deeper than a direct statement.
CREATE FUNCTION refuse_listing_source_label_event_change()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF TG_OP = 'DELETE' AND pg_trigger_depth() > 1 THEN
        RETURN OLD;
    END IF;
    RAISE EXCEPTION 'listing_source_label_events is append-only';
END;
$$;

CREATE TRIGGER listing_source_label_events_append_only
    BEFORE UPDATE OR DELETE ON listing_source_label_events
    FOR EACH ROW
    EXECUTE FUNCTION refuse_listing_source_label_event_change();

-- Seeds the history with every label that stands today, as it was set, so a later relabel keeps
-- the label and note it replaces.
INSERT INTO listing_source_label_events
    (listing_source_id, event_kind, source_label, source_label_note, actor, occurred_at)
SELECT listing_source_id, 'LABELLED', source_label, source_label_note,
       source_labelled_by, source_labelled_at
FROM listing_sources
WHERE source_label IS NOT NULL;
