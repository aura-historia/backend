-- Rebuildable, public-safe discovery state. Authoritative policies/observations stay private.
CREATE TABLE product_listing_shipping (
    product_listing_id uuid PRIMARY KEY REFERENCES product_listings(product_listing_id) ON DELETE CASCADE,
    dependency jsonb NOT NULL CHECK(jsonb_typeof(dependency)='object'),
    summary jsonb NOT NULL CHECK(jsonb_typeof(summary)='object'),
    valid_until timestamptz,
    projection_version bigint NOT NULL CHECK(projection_version>0)
);
CREATE INDEX product_listing_shipping_deadline_idx ON product_listing_shipping(valid_until) WHERE valid_until IS NOT NULL;
CREATE TABLE listing_shipping_dependencies (
    product_listing_id uuid NOT NULL REFERENCES product_listings(product_listing_id) ON DELETE CASCADE,
    dependency_kind text NOT NULL CHECK(dependency_kind IN ('SOURCE','PROFILE','LOCATION','RESOLUTION','PROVIDER')),
    dependency_id text NOT NULL CHECK(octet_length(dependency_id) BETWEEN 1 AND 255),
    PRIMARY KEY(product_listing_id,dependency_kind,dependency_id)
);
CREATE INDEX listing_shipping_dependencies_reverse_idx ON listing_shipping_dependencies(dependency_kind,dependency_id,product_listing_id);
CREATE TABLE listing_shipping_work (
    work_id uuid PRIMARY KEY,
    dependency_kind text NOT NULL CHECK(dependency_kind IN ('SOURCE','PROFILE','LISTING','LOCATION','RESOLUTION','PROVIDER')),
    dependency_id text NOT NULL CHECK(octet_length(dependency_id) BETWEEN 1 AND 255),
    generation bigint NOT NULL DEFAULT 1 CHECK(generation>0),
    after_listing uuid,
    lease_token uuid,
    lease_until timestamptz,
    pending boolean NOT NULL DEFAULT true,
    pending_since timestamptz NOT NULL DEFAULT now(),
    UNIQUE(dependency_kind,dependency_id),
    CHECK((lease_token IS NULL)=(lease_until IS NULL))
);
CREATE INDEX listing_shipping_work_pending_idx ON listing_shipping_work(pending_since,work_id) WHERE pending;
CREATE TABLE listing_shipping_wakeups (
    work_id uuid NOT NULL REFERENCES listing_shipping_work(work_id),
    generation bigint NOT NULL CHECK(generation>0),
    PRIMARY KEY(work_id,generation)
);
CREATE FUNCTION enqueue_listing_shipping(kind text,id text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE w uuid; g bigint;
BEGIN
    INSERT INTO listing_shipping_work(work_id,dependency_kind,dependency_id)
    VALUES(listing_geography_uuid_v7(),kind,id)
    ON CONFLICT(dependency_kind,dependency_id) DO UPDATE SET
        generation=listing_shipping_work.generation+1,after_listing=NULL,
        lease_token=NULL,lease_until=NULL,pending=true,
        pending_since=CASE WHEN listing_shipping_work.pending THEN listing_shipping_work.pending_since ELSE now() END
    RETURNING work_id,generation INTO w,g;
    INSERT INTO listing_shipping_wakeups VALUES(w,g);
END $$;
CREATE FUNCTION signal_listing_shipping() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_TABLE_NAME IN ('shipping_changes','shipping_validity_changes') THEN
        PERFORM enqueue_listing_shipping(CASE NEW.subject_kind WHEN 'SOURCE_DEFAULT' THEN 'SOURCE' ELSE NEW.subject_kind END,NEW.subject_id::text);
    ELSIF TG_TABLE_NAME='product_listing_events' THEN
        PERFORM enqueue_listing_shipping('LISTING',NEW.product_listing_id::text);
    ELSIF TG_TABLE_NAME='party_location_changes' THEN
        PERFORM enqueue_listing_shipping('LOCATION',NEW.party_location_id::text);
    ELSIF TG_TABLE_NAME='geo_resolution_changes' THEN
        IF NEW.subject_kind='PARTY_LOCATION' THEN
            PERFORM enqueue_listing_shipping('LOCATION',NEW.subject_id::text);
        ELSIF NEW.subject_kind IS NULL THEN
            PERFORM enqueue_listing_shipping('RESOLUTION',NEW.resolution_id::text);
        END IF;
    ELSIF TG_TABLE_NAME='geo_resolution_provider_controls' AND NEW.storage_permitted IS DISTINCT FROM OLD.storage_permitted THEN
        PERFORM enqueue_listing_shipping('PROVIDER',NEW.profile);
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER listing_shipping_management_signal AFTER INSERT ON shipping_changes FOR EACH ROW EXECUTE FUNCTION signal_listing_shipping();
CREATE TRIGGER listing_shipping_validity_signal AFTER INSERT ON shipping_validity_changes FOR EACH ROW EXECUTE FUNCTION signal_listing_shipping();
CREATE TRIGGER listing_shipping_listing_signal AFTER INSERT ON product_listing_events FOR EACH ROW WHEN(NEW.event_group='DOMAIN') EXECUTE FUNCTION signal_listing_shipping();
CREATE TRIGGER listing_shipping_location_signal AFTER INSERT ON party_location_changes FOR EACH ROW EXECUTE FUNCTION signal_listing_shipping();
CREATE TRIGGER listing_shipping_resolution_signal AFTER INSERT ON geo_resolution_changes FOR EACH ROW EXECUTE FUNCTION signal_listing_shipping();
CREATE TRIGGER listing_shipping_rights_signal AFTER UPDATE ON geo_resolution_provider_controls FOR EACH ROW EXECUTE FUNCTION signal_listing_shipping();

-- Canonical reverse input, including currently unavailable references so restoration can
-- invalidate them. Only identities leave this function, never sites, terms or evidence.
CREATE FUNCTION listing_shipping_references(ids uuid[]) RETURNS TABLE(product_listing_id uuid,kind text,id text)
LANGUAGE sql STABLE AS $$
    WITH listings AS (
        SELECT p.product_listing_id,p.listing_source_id,p.delivery_configuration,
            CASE WHEN p.delivery_configuration IS NULL OR p.delivery_configuration#>>'{selection,kind}'='INHERIT_SOURCE_DEFAULT' THEN d.shipping_profile_id
                 WHEN p.delivery_configuration#>>'{selection,kind}'='EXPLICIT' THEN (p.delivery_configuration#>>'{selection,profile_id}')::uuid END AS profile_id
        FROM product_listings p LEFT JOIN shipping_source_defaults d USING(listing_source_id)
        WHERE p.product_listing_id=ANY(ids)
    ), sites AS (
        SELECT l.product_listing_id,r.party_location_id FROM listings l
        JOIN shipping_profile_location_references r ON r.shipping_profile_id=l.profile_id
        UNION
        SELECT l.product_listing_id,(v.value#>>'{}')::uuid FROM listings l,
            LATERAL jsonb_path_query(l.delivery_configuration,'lax $.**.party_location_id') v(value)
    ), resolutions AS (
        SELECT s.product_listing_id,b.resolution_id,r.profile FROM sites s
        JOIN geo_resolution_bindings b ON b.subject_kind='PARTY_LOCATION' AND b.subject_id=s.party_location_id
        JOIN geo_resolutions r USING(resolution_id)
    )
    SELECT product_listing_id,'SOURCE',listing_source_id::text FROM listings
    UNION SELECT product_listing_id,'PROFILE',profile_id::text FROM listings WHERE profile_id IS NOT NULL
    UNION SELECT product_listing_id,'LOCATION',party_location_id::text FROM sites
    UNION SELECT product_listing_id,'RESOLUTION',resolution_id::text FROM resolutions
    UNION SELECT product_listing_id,'PROVIDER',profile FROM resolutions
$$;
ALTER TABLE product_listing_events DROP CONSTRAINT product_listing_events_type_group_check;
ALTER TABLE product_listing_events ADD CONSTRAINT product_listing_events_type_group_check CHECK (
    (event_group='DOMAIN' AND event_type IN ('PRODUCT_LISTING_DISCOVERED','PRODUCT_LISTING_CHANGED')) OR
    (event_group='ENRICHMENT' AND event_type IN ('ENRICHMENT_EMBEDDED','ENRICHMENT_TRANSLATED_TITLES','ENRICHMENT_GEOGRAPHY_UPDATED','ENRICHMENT_SHIPPING_UPDATED'))
);
-- Initial source traversal is bounded by workers; concurrent listings have independent work.
SELECT enqueue_listing_shipping('SOURCE',listing_source_id::text) FROM listing_sources;
