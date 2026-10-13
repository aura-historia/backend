-- Historical matches and pending per-match deliveries remain LEGACY (NULL run).
-- Quiesce matcher/notification writers while applying this migration and deploying
-- compatible binaries. No historical matches are enrolled or replayed.
CREATE TABLE search_filter_matching_runs (
    window_end timestamptz PRIMARY KEY,
    status text NOT NULL DEFAULT 'RUNNING' CHECK (status IN ('RUNNING', 'COMPLETED', 'INTERRUPTED')),
    created timestamptz NOT NULL DEFAULT now(),
    recovery_deadline timestamptz NOT NULL DEFAULT now() + INTERVAL '24 hours'
);
CREATE UNIQUE INDEX search_filter_matching_one_running ON search_filter_matching_runs (status) WHERE status = 'RUNNING';

CREATE TABLE search_filter_matching_run_filters (
    window_end timestamptz NOT NULL REFERENCES search_filter_matching_runs,
    user_id uuid NOT NULL REFERENCES users ON DELETE CASCADE,
    user_search_filter_id uuid NOT NULL,
    snapshot jsonb NOT NULL,
    terminal boolean NOT NULL DEFAULT false,
    failed_attempts integer NOT NULL DEFAULT 0 CHECK (failed_attempts >= 0),
    selected_listing_ids uuid[],
    selected_event_ids uuid[],
    CHECK ((selected_listing_ids IS NULL AND selected_event_ids IS NULL) OR (
        selected_listing_ids IS NOT NULL AND selected_event_ids IS NOT NULL
        AND cardinality(selected_listing_ids) = cardinality(selected_event_ids)
        AND cardinality(selected_listing_ids) <= 50
        AND array_position(selected_listing_ids, NULL) IS NULL
        AND array_position(selected_event_ids, NULL) IS NULL)),
    PRIMARY KEY (window_end, user_search_filter_id)
);
CREATE INDEX search_filter_matching_run_user ON search_filter_matching_run_filters(window_end, user_id, terminal);

CREATE TABLE saved_search_digest_groups (
    window_end timestamptz NOT NULL REFERENCES search_filter_matching_runs,
    user_id uuid NOT NULL REFERENCES users ON DELETE CASCADE,
    finalized_at timestamptz,
    PRIMARY KEY (window_end, user_id)
);

ALTER TABLE search_filter_matches ADD COLUMN matching_run_end timestamptz REFERENCES search_filter_matching_runs;
ALTER TABLE search_filter_matches ADD COLUMN notification_selection_complete boolean NOT NULL DEFAULT false;
ALTER TABLE search_filter_matches ADD COLUMN notification_recovery_after timestamptz NOT NULL DEFAULT now();
CREATE INDEX search_filter_match_pending_selection ON search_filter_matches(notification_recovery_after, created, origin_event_id, user_search_filter_id, product_listing_id)
    WHERE matching_run_end IS NOT NULL AND NOT notification_selection_complete;

-- Membership snapshots outlive deletion of an in-app notification. The match key
-- is retained as the notification fence; a finalized group cannot gain members.
CREATE TABLE saved_search_digests (
    digest_id uuid PRIMARY KEY,
    window_end timestamptz NOT NULL,
    user_id uuid NOT NULL,
    payload_version smallint NOT NULL DEFAULT 1 CHECK (payload_version = 1),
    created timestamptz NOT NULL DEFAULT now(),
    UNIQUE (window_end, user_id),
    FOREIGN KEY (window_end, user_id) REFERENCES saved_search_digest_groups ON DELETE CASCADE
);
CREATE TABLE saved_search_digest_members (
    user_id uuid NOT NULL REFERENCES users ON DELETE CASCADE,
    user_search_filter_id uuid NOT NULL,
    product_listing_id uuid NOT NULL,
    origin_event_id uuid NOT NULL,
    notification_id uuid NOT NULL UNIQUE,
    matching_run_end timestamptz NOT NULL REFERENCES search_filter_matching_runs,
    digest_id uuid REFERENCES saved_search_digests ON DELETE CASCADE,
    snapshot jsonb NOT NULL CHECK (jsonb_typeof(snapshot) = 'object'),
    created timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, user_search_filter_id, product_listing_id),
    FOREIGN KEY (matching_run_end, user_id) REFERENCES saved_search_digest_groups ON DELETE CASCADE
);
CREATE INDEX saved_search_digest_unassigned ON saved_search_digest_members(user_id, matching_run_end, created)
    WHERE digest_id IS NULL;
CREATE INDEX saved_search_digest_membership ON saved_search_digest_members(digest_id, created, origin_event_id, user_search_filter_id, product_listing_id);

ALTER TABLE notification_deliveries ALTER COLUMN notification_id DROP NOT NULL;
ALTER TABLE notification_deliveries ADD COLUMN digest_id uuid UNIQUE REFERENCES saved_search_digests ON DELETE CASCADE;
ALTER TABLE notification_deliveries ADD CONSTRAINT notification_delivery_source_check
    CHECK ((notification_id IS NOT NULL) <> (digest_id IS NOT NULL));

-- Fence incompatible generators even if an old CDC record is replayed during cutover.
CREATE FUNCTION fence_saved_search_individual_email() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.notification_id IS NOT NULL AND EXISTS (
        SELECT 1 FROM notifications n JOIN search_filter_matches m
          ON (m.user_id, m.user_search_filter_id, m.product_listing_id, m.origin_event_id) =
             (n.user_id, n.user_search_filter_id, n.product_listing_id, n.origin_event_id)
        WHERE n.notification_id = NEW.notification_id AND m.matching_run_end IS NOT NULL
    ) THEN
        RAISE EXCEPTION 'saved-search run matches require digest delivery';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER fence_saved_search_individual_email BEFORE INSERT ON notification_deliveries
    FOR EACH ROW EXECUTE FUNCTION fence_saved_search_individual_email();

CREATE VIEW saved_search_digest_status AS
SELECT g.window_end, g.user_id, r.status AS run_status, r.recovery_deadline,
       g.finalized_at, d.digest_id, delivery.notification_delivery_id,
       delivery.status AS delivery_status, delivery.attempt_count, delivery.last_error_code,
       EXTRACT(EPOCH FROM now() - r.created) AS age_seconds,
       (SELECT count(*) FROM search_filter_matching_run_filters f WHERE f.window_end = g.window_end AND f.user_id = g.user_id AND NOT f.terminal) AS filters_pending,
       (SELECT count(*) FROM search_filter_matches m WHERE m.matching_run_end = g.window_end AND m.user_id = g.user_id AND NOT m.notification_selection_complete) AS selections_pending,
       (SELECT count(*) FROM saved_search_digest_members m WHERE m.digest_id = d.digest_id) AS eligible_matches,
       (SELECT count(DISTINCT product_listing_id) FROM saved_search_digest_members m WHERE m.digest_id = d.digest_id) AS deduplicated_listings
FROM saved_search_digest_groups g JOIN search_filter_matching_runs r USING(window_end)
LEFT JOIN saved_search_digests d USING(window_end, user_id)
LEFT JOIN notification_deliveries delivery ON delivery.digest_id = d.digest_id;
