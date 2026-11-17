-- Trigger-fed delivery outbox (docs/events/flow.md#cdc-routing). It replaces DMS, Kinesis and
-- the CDC router once the runtime moves to the self-hosted host: an AFTER INSERT trigger on
-- every selected source appends a compact snapshot in the writer's own transaction, and the
-- outbox dispatcher (src/delivery-outbox) routes it into the job queue and deletes it in one
-- transaction. This migration must reach a database only together with a running dispatcher;
-- without one the outbox only grows.
--
-- Snapshots carry exactly the columns their route reads: compact identities, generations and
-- the ProductListing event payload. Raw evidence, addresses, email addresses, provider data
-- and lease state never enter the outbox. Neither table is a capture source itself.

---------------------------------------------------------------------------------------------
-- Pending snapshots
---------------------------------------------------------------------------------------------

CREATE TABLE delivery_outbox (
    -- Dispatch order. Custody keeps the value and redrive restores it.
    outbox_id bigserial PRIMARY KEY,
    source_table text NOT NULL,
    -- Version of the snapshot shape; the dispatcher rejects versions it does not know.
    snapshot_version smallint NOT NULL,
    snapshot jsonb NOT NULL,
    enqueued_at timestamptz NOT NULL DEFAULT now()
);

-- Every dispatch deletes the rows it claimed, so dead tuples accumulate quickly while the live
-- table stays small. Vacuum and analyze after a fixed number of changes, without throttling.
ALTER TABLE delivery_outbox SET (
    autovacuum_vacuum_scale_factor = 0,
    autovacuum_vacuum_threshold = 5000,
    autovacuum_vacuum_insert_scale_factor = 0,
    autovacuum_vacuum_insert_threshold = 5000,
    autovacuum_analyze_scale_factor = 0,
    autovacuum_analyze_threshold = 5000,
    autovacuum_vacuum_cost_delay = 0
);

-- Wakes the listening dispatcher once a capturing transaction commits. PostgreSQL folds the
-- identical notifications of one transaction into one.
CREATE FUNCTION aura_delivery_outbox_notify()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    PERFORM pg_notify('delivery_outbox', '');
    RETURN NULL;
END
$$;

CREATE TRIGGER delivery_outbox_notify
    AFTER INSERT ON delivery_outbox
    FOR EACH STATEMENT
    EXECUTE FUNCTION aura_delivery_outbox_notify();

---------------------------------------------------------------------------------------------
-- Failure custody
---------------------------------------------------------------------------------------------

-- A snapshot the dispatcher cannot decode, route or encode moves here unchanged, in the
-- dispatch transaction that gave it up. Rows are never purged; operators redrive them into
-- delivery_outbox under their original ID once the cause is repaired.
CREATE TABLE delivery_outbox_failures (
    outbox_id bigint PRIMARY KEY,
    source_table text NOT NULL,
    snapshot_version smallint NOT NULL,
    snapshot jsonb NOT NULL,
    enqueued_at timestamptz NOT NULL,
    -- Bounded label of the failure; never an error message or snapshot content.
    error_class text NOT NULL,
    failed_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT delivery_outbox_failures_error_class_check
        CHECK (error_class ~ '^[a-z0-9_]{1,64}$')
);

---------------------------------------------------------------------------------------------
-- Capture
---------------------------------------------------------------------------------------------

-- One function serves every selected source. Each branch names its columns explicitly, so a
-- column added to a source later never enters the outbox without a reviewed change here and
-- in the dispatcher's codec.
CREATE FUNCTION aura_delivery_outbox_capture()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    captured jsonb;
BEGIN
    CASE TG_TABLE_NAME
        WHEN 'product_listing_events' THEN
            captured := jsonb_build_object(
                'event_id', NEW.event_id,
                'product_listing_id', NEW.product_listing_id,
                'event_type', NEW.event_type,
                'event_group', NEW.event_group,
                'event_type_schema_version', NEW.event_type_schema_version,
                'payload', NEW.payload);
        WHEN 'product_listing_raw_revisions' THEN
            captured := jsonb_build_object(
                'product_listing_raw_stream_id', NEW.product_listing_raw_stream_id,
                'product_listing_raw_revision_id', NEW.product_listing_raw_revision_id,
                'revision', NEW.revision);
        WHEN 'search_filter_matches' THEN
            captured := jsonb_build_object(
                'user_id', NEW.user_id,
                'user_search_filter_id', NEW.user_search_filter_id,
                'product_listing_id', NEW.product_listing_id,
                'origin_event_id', NEW.origin_event_id);
        WHEN 'notification_deliveries' THEN
            captured := jsonb_build_object(
                'notification_delivery_id', NEW.notification_delivery_id);
        WHEN 'marketing_email_consent_sync_intents' THEN
            captured := jsonb_build_object(
                'intent_id', NEW.intent_id,
                'recipient_key', NEW.recipient_key);
        WHEN 'geo_resolution_wakeups' THEN
            captured := jsonb_build_object(
                'resolution_id', NEW.resolution_id,
                'generation', NEW.generation);
        WHEN 'listing_geography_wakeups', 'listing_shipping_wakeups' THEN
            captured := jsonb_build_object(
                'work_id', NEW.work_id,
                'generation', NEW.generation);
        WHEN 'user_identity_deletion_requests' THEN
            captured := jsonb_build_object(
                'user_id', NEW.user_id);
    END CASE;
    INSERT INTO delivery_outbox (source_table, snapshot_version, snapshot)
    VALUES (TG_TABLE_NAME, 1, captured);
    RETURN NULL;
END
$$;

CREATE TRIGGER product_listing_events_delivery_outbox
    AFTER INSERT ON product_listing_events
    FOR EACH ROW EXECUTE FUNCTION aura_delivery_outbox_capture();

CREATE TRIGGER product_listing_raw_revisions_delivery_outbox
    AFTER INSERT ON product_listing_raw_revisions
    FOR EACH ROW EXECUTE FUNCTION aura_delivery_outbox_capture();

CREATE TRIGGER search_filter_matches_delivery_outbox
    AFTER INSERT ON search_filter_matches
    FOR EACH ROW EXECUTE FUNCTION aura_delivery_outbox_capture();

CREATE TRIGGER notification_deliveries_delivery_outbox
    AFTER INSERT ON notification_deliveries
    FOR EACH ROW EXECUTE FUNCTION aura_delivery_outbox_capture();

CREATE TRIGGER marketing_email_consent_sync_intents_delivery_outbox
    AFTER INSERT ON marketing_email_consent_sync_intents
    FOR EACH ROW EXECUTE FUNCTION aura_delivery_outbox_capture();

CREATE TRIGGER geo_resolution_wakeups_delivery_outbox
    AFTER INSERT ON geo_resolution_wakeups
    FOR EACH ROW EXECUTE FUNCTION aura_delivery_outbox_capture();

CREATE TRIGGER listing_geography_wakeups_delivery_outbox
    AFTER INSERT ON listing_geography_wakeups
    FOR EACH ROW EXECUTE FUNCTION aura_delivery_outbox_capture();

CREATE TRIGGER listing_shipping_wakeups_delivery_outbox
    AFTER INSERT ON listing_shipping_wakeups
    FOR EACH ROW EXECUTE FUNCTION aura_delivery_outbox_capture();

CREATE TRIGGER user_identity_deletion_requests_delivery_outbox
    AFTER INSERT ON user_identity_deletion_requests
    FOR EACH ROW EXECUTE FUNCTION aura_delivery_outbox_capture();
