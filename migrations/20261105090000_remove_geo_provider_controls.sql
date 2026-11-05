-- Geocoding is always on: the reviewed provider profile has no operator switch, storage
-- permission flag, daily/scope budget or concurrency limit. The provider row is used only for
-- the profile identity, the default marker and the automatic provider-health circuit breaker.
--
-- Expand/contract: binaries from the previous release still read and update the retired
-- control columns and scope budgets until rollout completes, so this migration pins them to
-- always-on values instead of dropping them. A later release drops them.

-- Evidence becomes eligible wherever storage was not permitted. Re-project the listings that
-- depend on such a profile, as the retired storage-permission triggers did.
SELECT enqueue_listing_geography('RESOLUTION',r.resolution_id::text)
FROM geo_resolution_provider_controls c JOIN geo_resolutions r USING(profile)
WHERE NOT c.storage_permitted AND EXISTS(SELECT 1 FROM geo_resolution_bindings b
    WHERE b.resolution_id=r.resolution_id AND b.subject_kind='PARTY_LOCATION')
ORDER BY r.resolution_id;
SELECT enqueue_listing_shipping('RESOLUTION',r.resolution_id::text)
FROM geo_resolution_provider_controls c JOIN geo_resolutions r USING(profile)
WHERE NOT c.storage_permitted AND EXISTS(SELECT 1 FROM geo_resolution_bindings b
    WHERE b.resolution_id=r.resolution_id AND b.subject_kind='PARTY_LOCATION')
ORDER BY r.resolution_id;

-- Storage permission is no longer a dependency of listing projections.
DROP TRIGGER listing_geography_provider_signal ON geo_resolution_provider_controls;
DROP TRIGGER listing_shipping_rights_signal ON geo_resolution_provider_controls;

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
    ELSIF TG_TABLE_NAME='product_listing_events' AND NEW.event_group='DOMAIN' THEN
        PERFORM enqueue_listing_geography('LISTING',NEW.product_listing_id::text);
    END IF;
    RETURN NEW;
END $$;

CREATE OR REPLACE FUNCTION signal_listing_shipping() RETURNS trigger LANGUAGE plpgsql AS $$
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
    END IF;
    RETURN NEW;
END $$;

-- Hand unfinished provider-wide work to the per-resolution work it fans out to, so no
-- pending re-projection is lost, then remove the provider dependency kinds.
SELECT enqueue_listing_geography('RESOLUTION',r.resolution_id::text)
FROM listing_geography_work w JOIN geo_resolutions r ON r.profile=w.dependency_id
WHERE w.dependency_kind='PROFILE' AND w.pending AND EXISTS(SELECT 1 FROM geo_resolution_bindings b
    WHERE b.resolution_id=r.resolution_id AND b.subject_kind='PARTY_LOCATION')
ORDER BY r.resolution_id;
SELECT enqueue_listing_shipping('RESOLUTION',r.resolution_id::text)
FROM listing_shipping_work w JOIN geo_resolutions r ON r.profile=w.dependency_id
WHERE w.dependency_kind='PROVIDER' AND w.pending AND EXISTS(SELECT 1 FROM geo_resolution_bindings b
    WHERE b.resolution_id=r.resolution_id AND b.subject_kind='PARTY_LOCATION')
ORDER BY r.resolution_id;

DELETE FROM listing_geography_wakeups WHERE work_id IN
    (SELECT work_id FROM listing_geography_work WHERE dependency_kind='PROFILE');
DELETE FROM listing_geography_work WHERE dependency_kind='PROFILE';
DELETE FROM listing_shipping_wakeups WHERE work_id IN
    (SELECT work_id FROM listing_shipping_work WHERE dependency_kind='PROVIDER');
DELETE FROM listing_shipping_work WHERE dependency_kind='PROVIDER';
DELETE FROM listing_shipping_dependencies WHERE dependency_kind='PROVIDER';

CREATE OR REPLACE FUNCTION listing_shipping_references(ids uuid[]) RETURNS TABLE(product_listing_id uuid,kind text,id text)
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
        SELECT s.product_listing_id,b.resolution_id FROM sites s
        JOIN geo_resolution_bindings b ON b.subject_kind='PARTY_LOCATION' AND b.subject_id=s.party_location_id
    )
    SELECT product_listing_id,'SOURCE',listing_source_id::text FROM listings
    UNION SELECT product_listing_id,'PROFILE',profile_id::text FROM listings WHERE profile_id IS NOT NULL
    UNION SELECT product_listing_id,'LOCATION',party_location_id::text FROM sites
    UNION SELECT product_listing_id,'RESOLUTION',resolution_id::text FROM resolutions
$$;

ALTER TABLE listing_geography_work DROP CONSTRAINT listing_geography_work_dependency_kind_check,
    ADD CONSTRAINT listing_geography_work_dependency_kind_check
        CHECK (dependency_kind IN ('SOURCE','LOCATION','RESOLUTION','LISTING'));
ALTER TABLE listing_shipping_dependencies DROP CONSTRAINT listing_shipping_dependencies_dependency_kind_check,
    ADD CONSTRAINT listing_shipping_dependencies_dependency_kind_check
        CHECK (dependency_kind IN ('SOURCE','PROFILE','LOCATION','RESOLUTION'));
ALTER TABLE listing_shipping_work DROP CONSTRAINT listing_shipping_work_dependency_kind_check,
    ADD CONSTRAINT listing_shipping_work_dependency_kind_check
        CHECK (dependency_kind IN ('SOURCE','PROFILE','LISTING','LOCATION','RESOLUTION'));

-- Retired controls: always on and effectively unlimited for previous-release binaries.
UPDATE geo_resolution_provider_controls SET enabled=true,storage_permitted=true,
    daily_limit=100000,scope_daily_limit=100000,concurrency_limit=8,scope_concurrency_limit=8;
ALTER TABLE geo_resolution_provider_controls
    ALTER COLUMN enabled SET DEFAULT true,
    ALTER COLUMN storage_permitted SET DEFAULT true,
    ALTER COLUMN daily_limit SET DEFAULT 100000,
    ALTER COLUMN scope_daily_limit SET DEFAULT 100000,
    ALTER COLUMN concurrency_limit SET DEFAULT 8,
    ALTER COLUMN scope_concurrency_limit SET DEFAULT 8;

COMMENT ON TABLE geo_resolution_provider_controls IS
    'Geocoding provider profiles: identity, the default marker and automatic circuit-breaker state. enabled, storage_permitted, the limits, budget_day and calls are retired and unread; they remain only until the previous release is gone.';
COMMENT ON TABLE geo_resolution_scope_budgets IS
    'Retired and unread; remains only until the previous release is gone.';
