-- Bounded derived read state. Addresses/evidence remain at their owning authority.
CREATE TABLE product_listing_geographies (
    product_listing_id uuid PRIMARY KEY REFERENCES product_listings(product_listing_id) ON DELETE CASCADE,
    dependency jsonb NOT NULL,
    geography jsonb NOT NULL,
    valid_until timestamptz,
    geography_version bigint NOT NULL CHECK (geography_version > 0),
    indexed_version bigint NOT NULL DEFAULT 0 CHECK (indexed_version >= 0),
    blocks_queries boolean NOT NULL DEFAULT false
);
CREATE INDEX product_listing_geographies_unindexed_idx ON product_listing_geographies(product_listing_id) WHERE indexed_version < geography_version;
CREATE INDEX product_listing_geographies_barrier_idx ON product_listing_geographies(product_listing_id) WHERE blocks_queries AND indexed_version < geography_version;
CREATE INDEX product_listing_geographies_expiry_idx ON product_listing_geographies(valid_until) WHERE valid_until IS NOT NULL;
CREATE INDEX product_listings_source_geography_cursor_idx ON product_listings(listing_source_id,product_listing_id);

-- Leased, coalesced dependency work. Claims never span external I/O; pages and
-- checkpoints commit together. A newer invalidation resets both cursors and fences claims.
CREATE TABLE listing_geography_work (
    work_id uuid PRIMARY KEY,
    dependency_kind text NOT NULL CHECK (dependency_kind IN ('SOURCE','LOCATION','RESOLUTION','LISTING','PROFILE')),
    dependency_id text NOT NULL CHECK (octet_length(dependency_id) BETWEEN 1 AND 255),
    generation bigint NOT NULL DEFAULT 1 CHECK (generation > 0),
    after_source uuid,
    after_listing uuid,
    lease_token uuid,
    lease_until timestamptz,
    pending boolean NOT NULL DEFAULT true,
    blocks_queries boolean NOT NULL DEFAULT true CHECK (blocks_queries OR dependency_kind='LISTING'),
    pending_since timestamptz NOT NULL DEFAULT now(),
    UNIQUE(dependency_kind,dependency_id),
    CHECK ((after_source IS NULL) = (after_listing IS NULL)),
    CHECK ((lease_token IS NULL) = (lease_until IS NULL))
);
CREATE INDEX listing_geography_work_pending_idx ON listing_geography_work(pending_since) WHERE pending;
CREATE INDEX listing_geography_work_barrier_idx ON listing_geography_work(work_id) WHERE pending AND blocks_queries;
CREATE TABLE listing_geography_wakeups (
    work_id uuid NOT NULL REFERENCES listing_geography_work(work_id),
    generation bigint NOT NULL CHECK (generation > 0),
    PRIMARY KEY(work_id,generation)
);
-- Query-wide fail-closed epoch. Serializes short invalidations, never listing fanout.
CREATE TABLE listing_geography_gate (singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton), epoch bigint NOT NULL DEFAULT 1 CHECK(epoch > 0));
INSERT INTO listing_geography_gate DEFAULT VALUES;

-- PostgreSQL 16 has no UUIDv7 generator. Use database time + random RFC4122 bits
-- only for technical wake-up identity, preserving the workspace's UUIDv7 contract.
CREATE FUNCTION listing_geography_uuid_v7() RETURNS uuid LANGUAGE sql VOLATILE AS $$
    SELECT (lpad(to_hex((extract(epoch FROM clock_timestamp())*1000)::bigint),12,'0') ||
        '7' || substr(replace(gen_random_uuid()::text,'-',''),14,3) ||
        '8' || substr(replace(gen_random_uuid()::text,'-',''),18,15))::uuid
$$;
CREATE FUNCTION enqueue_listing_geography(kind text, id text, blocking boolean DEFAULT true) RETURNS void LANGUAGE plpgsql AS $$
DECLARE w uuid; g bigint;
BEGIN
    INSERT INTO listing_geography_work(work_id,dependency_kind,dependency_id,blocks_queries)
    VALUES(listing_geography_uuid_v7(),kind,id,blocking)
    ON CONFLICT(dependency_kind,dependency_id) DO UPDATE SET
        generation=listing_geography_work.generation+1,after_source=NULL,after_listing=NULL,
        lease_token=NULL,lease_until=NULL,pending=true,
        blocks_queries=CASE WHEN listing_geography_work.pending THEN listing_geography_work.blocks_queries OR EXCLUDED.blocks_queries ELSE EXCLUDED.blocks_queries END,
        pending_since=CASE WHEN listing_geography_work.pending THEN listing_geography_work.pending_since ELSE now() END
    RETURNING work_id,generation INTO w,g;
    INSERT INTO listing_geography_wakeups VALUES(w,g);
    IF blocking THEN
        UPDATE listing_geography_gate SET epoch=epoch+1;
    END IF;
END $$;
CREATE FUNCTION signal_listing_geography() RETURNS trigger LANGUAGE plpgsql AS $$
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
        -- Ordinary canonical catch-up/creation cannot revoke permitted geography.
        -- Lifecycle changes remain blocking through removal/tombstone visibility.
        -- Future listing-location writes (G11) must extend this classification.
        PERFORM enqueue_listing_geography('LISTING',NEW.product_listing_id::text,
            NEW.event_type_schema_version<>1 OR NEW.payload ? 'lifecycle');
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER listing_geography_default_signal AFTER INSERT ON listing_source_location_default_changes FOR EACH ROW EXECUTE FUNCTION signal_listing_geography();
CREATE TRIGGER listing_geography_location_signal AFTER INSERT ON party_location_changes FOR EACH ROW EXECUTE FUNCTION signal_listing_geography();
CREATE TRIGGER listing_geography_resolution_signal AFTER INSERT ON geo_resolution_changes FOR EACH ROW EXECUTE FUNCTION signal_listing_geography();
CREATE TRIGGER listing_geography_provider_signal AFTER UPDATE ON geo_resolution_provider_controls FOR EACH ROW EXECUTE FUNCTION signal_listing_geography();
CREATE TRIGGER listing_geography_listing_signal AFTER INSERT ON product_listing_events FOR EACH ROW WHEN (NEW.event_group='DOMAIN') EXECUTE FUNCTION signal_listing_geography();

ALTER TABLE product_listing_events DROP CONSTRAINT product_listing_events_type_group_check;
ALTER TABLE product_listing_events ADD CONSTRAINT product_listing_events_type_group_check CHECK (
    (event_group='DOMAIN' AND event_type IN ('PRODUCT_LISTING_DISCOVERED','PRODUCT_LISTING_CHANGED')) OR
    (event_group='ENRICHMENT' AND event_type IN ('ENRICHMENT_EMBEDDED','ENRICHMENT_TRANSLATED_TITLES','ENRICHMENT_GEOGRAPHY_UPDATED'))
);
-- Initial rebuild uses source-level work, not one synchronous rewrite per listing.
SELECT enqueue_listing_geography('SOURCE',listing_source_id::text) FROM listing_sources;
