-- Geographic discovery accepts ordinary read-projection lag. Invalidation work
-- and projection versions remain durable; they no longer block any public query.
CREATE FUNCTION enqueue_listing_geography(kind text, id text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE w uuid; g bigint;
BEGIN
    INSERT INTO listing_geography_work(work_id,dependency_kind,dependency_id)
    VALUES(listing_geography_uuid_v7(),kind,id)
    ON CONFLICT(dependency_kind,dependency_id) DO UPDATE SET
        generation=listing_geography_work.generation+1,after_source=NULL,after_listing=NULL,
        lease_token=NULL,lease_until=NULL,pending=true,
        pending_since=CASE WHEN listing_geography_work.pending THEN listing_geography_work.pending_since ELSE now() END
    RETURNING work_id,generation INTO w,g;
    INSERT INTO listing_geography_wakeups VALUES(w,g);
END $$;

CREATE OR REPLACE FUNCTION signal_listing_geography() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_TABLE_NAME='listing_source_location_default_changes' THEN
        PERFORM enqueue_listing_geography('SOURCE',NEW.listing_source_id::text);
    ELSIF TG_TABLE_NAME='party_location_changes' THEN
        PERFORM enqueue_listing_geography('LOCATION',NEW.party_location_id::text);
    ELSIF TG_TABLE_NAME='geo_resolution_changes' THEN
        IF NEW.subject_kind='PARTY_LOCATION' THEN
            PERFORM enqueue_listing_geography('LOCATION',NEW.subject_id::text);
        ELSIF NEW.subject_kind IS NULL THEN
            PERFORM enqueue_listing_geography('RESOLUTION',NEW.resolution_id::text);
        END IF;
    ELSIF TG_TABLE_NAME='geo_resolution_provider_controls' THEN
        IF NEW.storage_permitted IS DISTINCT FROM OLD.storage_permitted THEN
            PERFORM enqueue_listing_geography('PROFILE',NEW.profile);
        END IF;
    ELSIF TG_TABLE_NAME='product_listing_events' AND NEW.event_group='DOMAIN' THEN
        PERFORM enqueue_listing_geography('LISTING',NEW.product_listing_id::text);
    END IF;
    RETURN NEW;
END $$;

DROP FUNCTION enqueue_listing_geography(text, text, boolean);
DROP INDEX product_listing_geographies_barrier_idx;
DROP INDEX listing_geography_work_barrier_idx;
ALTER TABLE product_listing_geographies DROP COLUMN blocks_queries;
ALTER TABLE listing_geography_work DROP COLUMN blocks_queries;
DROP TABLE listing_geography_gate;
