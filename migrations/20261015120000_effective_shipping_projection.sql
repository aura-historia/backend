-- Rebuildable, public-safe summaries; private inputs and checkpoints are never CDC sources.
CREATE TABLE product_listing_shipping (
    product_listing_id uuid PRIMARY KEY REFERENCES product_listings(product_listing_id) ON DELETE CASCADE,
    dependency jsonb NOT NULL CHECK (jsonb_typeof(dependency)='object'),
    summary jsonb NOT NULL CHECK (jsonb_typeof(summary)='object'),
    valid_until timestamptz,
    shipping_version bigint NOT NULL CHECK (shipping_version > 0)
);
CREATE INDEX product_listing_shipping_expiry_idx ON product_listing_shipping(valid_until) WHERE valid_until IS NOT NULL;
CREATE TABLE product_listing_shipping_dependencies (
    product_listing_id uuid NOT NULL REFERENCES product_listings(product_listing_id) ON DELETE CASCADE,
    dependency_kind text NOT NULL CHECK (dependency_kind IN ('SOURCE','PROFILE','LOCATION','RESOLUTION')),
    dependency_id uuid NOT NULL,
    PRIMARY KEY (product_listing_id,dependency_kind,dependency_id)
);
CREATE INDEX product_listing_shipping_reverse_idx ON product_listing_shipping_dependencies(dependency_kind,dependency_id,product_listing_id);
CREATE TABLE listing_shipping_work (
    work_id uuid PRIMARY KEY,
    dependency_kind text NOT NULL CHECK (dependency_kind IN ('SOURCE','PROFILE','LOCATION','RESOLUTION','PROVIDER','LISTING')),
    dependency_id text NOT NULL CHECK (octet_length(dependency_id) BETWEEN 1 AND 255),
    generation bigint NOT NULL DEFAULT 1 CHECK (generation > 0),
    after_listing uuid,
    lease_token uuid, lease_until timestamptz,
    pending boolean NOT NULL DEFAULT true,
    pending_since timestamptz NOT NULL DEFAULT now(),
    UNIQUE(dependency_kind,dependency_id),
    CHECK ((lease_token IS NULL) = (lease_until IS NULL))
);
CREATE INDEX listing_shipping_work_pending_idx ON listing_shipping_work(pending_since,work_id) WHERE pending;
CREATE TABLE listing_shipping_wakeups (
    work_id uuid NOT NULL REFERENCES listing_shipping_work(work_id),
    generation bigint NOT NULL CHECK (generation > 0),
    PRIMARY KEY(work_id,generation)
);
CREATE FUNCTION enqueue_listing_shipping(kind text, id text) RETURNS void LANGUAGE plpgsql AS $$
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
    IF TG_TABLE_NAME='shipping_changes' THEN
        IF NEW.schema_version<>1 THEN RAISE EXCEPTION 'unsupported shipping change schema'; END IF;
        PERFORM enqueue_listing_shipping(CASE WHEN NEW.subject_kind='PROFILE' THEN 'PROFILE' ELSE 'SOURCE' END, NEW.subject_id::text);
    ELSIF TG_TABLE_NAME='party_location_changes' THEN
        PERFORM enqueue_listing_shipping('LOCATION',NEW.party_location_id::text);
    ELSIF TG_TABLE_NAME='geo_resolution_changes' THEN
        IF NEW.subject_kind='PARTY_LOCATION' THEN
            PERFORM enqueue_listing_shipping('LOCATION',NEW.subject_id::text);
        ELSIF NEW.subject_kind IS NULL THEN
            PERFORM enqueue_listing_shipping('RESOLUTION',NEW.resolution_id::text);
        END IF;
    ELSIF TG_TABLE_NAME='geo_resolution_provider_controls' THEN
        IF NEW.storage_permitted IS DISTINCT FROM OLD.storage_permitted THEN
            PERFORM enqueue_listing_shipping('PROVIDER',NEW.profile);
        END IF;
    ELSIF TG_TABLE_NAME='product_listings' THEN
        -- Observation-only validity changes need no canonical event/revision.
        IF TG_OP='INSERT' OR
            (NEW.listing_source_id,NEW.lifecycle,NEW.delivery_configuration,NEW.delivery_verified_at,NEW.delivery_fresh_until,NEW.delivery_expires_at)
            IS DISTINCT FROM
            (OLD.listing_source_id,OLD.lifecycle,OLD.delivery_configuration,OLD.delivery_verified_at,OLD.delivery_fresh_until,OLD.delivery_expires_at) THEN
            PERFORM enqueue_listing_shipping('LISTING',NEW.product_listing_id::text);
        END IF;
    ELSIF TG_TABLE_NAME='shipping_profiles' THEN
        IF (NEW.verified_at,NEW.fresh_until,NEW.expires_at,NEW.integration_method,NEW.integration_domain,NEW.current_source_id)
            IS DISTINCT FROM (OLD.verified_at,OLD.fresh_until,OLD.expires_at,OLD.integration_method,OLD.integration_domain,OLD.current_source_id) THEN
            PERFORM enqueue_listing_shipping('PROFILE',NEW.shipping_profile_id::text);
        END IF;
    ELSE
        IF TG_OP='DELETE' THEN
            PERFORM enqueue_listing_shipping('SOURCE',OLD.listing_source_id::text);
        ELSE
            PERFORM enqueue_listing_shipping('SOURCE',NEW.listing_source_id::text);
        END IF;
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER listing_shipping_management AFTER INSERT ON shipping_changes FOR EACH ROW EXECUTE FUNCTION signal_listing_shipping();
CREATE TRIGGER listing_shipping_location AFTER INSERT ON party_location_changes FOR EACH ROW EXECUTE FUNCTION signal_listing_shipping();
CREATE TRIGGER listing_shipping_resolution AFTER INSERT ON geo_resolution_changes FOR EACH ROW EXECUTE FUNCTION signal_listing_shipping();
CREATE TRIGGER listing_shipping_rights AFTER UPDATE ON geo_resolution_provider_controls FOR EACH ROW EXECUTE FUNCTION signal_listing_shipping();
CREATE TRIGGER listing_shipping_listing AFTER INSERT OR UPDATE ON product_listings FOR EACH ROW EXECUTE FUNCTION signal_listing_shipping();
CREATE TRIGGER listing_shipping_profile_validity AFTER UPDATE ON shipping_profiles FOR EACH ROW EXECUTE FUNCTION signal_listing_shipping();
CREATE TRIGGER listing_shipping_source AFTER UPDATE ON listing_sources FOR EACH ROW EXECUTE FUNCTION signal_listing_shipping();
CREATE TRIGGER listing_shipping_methods AFTER INSERT OR UPDATE OR DELETE ON listing_source_ingestion_methods FOR EACH ROW EXECUTE FUNCTION signal_listing_shipping();
CREATE TRIGGER listing_shipping_shopify AFTER INSERT OR UPDATE OR DELETE ON listing_source_shopify_ingestion_configurations FOR EACH ROW EXECUTE FUNCTION signal_listing_shipping();
CREATE TRIGGER listing_shipping_woocommerce AFTER INSERT OR UPDATE OR DELETE ON listing_source_woocommerce_ingestion_configurations FOR EACH ROW EXECUTE FUNCTION signal_listing_shipping();
ALTER TABLE product_listing_events DROP CONSTRAINT product_listing_events_type_group_check;
ALTER TABLE product_listing_events ADD CONSTRAINT product_listing_events_type_group_check CHECK (
    (event_group='DOMAIN' AND event_type IN ('PRODUCT_LISTING_DISCOVERED','PRODUCT_LISTING_CHANGED')) OR
    (event_group='ENRICHMENT' AND event_type IN ('ENRICHMENT_EMBEDDED','ENRICHMENT_TRANSLATED_TITLES','ENRICHMENT_GEOGRAPHY_UPDATED','ENRICHMENT_SHIPPING_UPDATED'))
);
-- Source traversal is resumable; concurrent/new assignments have independent listing work.
SELECT enqueue_listing_shipping('SOURCE',listing_source_id::text) FROM listing_sources;
