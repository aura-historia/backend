-- Geocoding evidence has no time limit (docs/geography.md, durable resolution), so no deadline
-- is read any more. Dropping each column also drops its expiry index and, for resolutions, the
-- evidence/deadline shape CHECK.
ALTER TABLE geo_resolutions DROP COLUMN valid_until;
ALTER TABLE product_listing_geographies DROP COLUMN valid_until;

-- Private address erasure no longer clears a deadline beside the erased evidence.
CREATE OR REPLACE FUNCTION erase_private_address_resolution() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    old_resolution uuid;
BEGIN
    IF TG_OP='UPDATE' AND NEW.geography IS NOT DISTINCT FROM OLD.geography AND NOT NEW.deleted THEN
        RETURN NEW;
    END IF;
    SELECT resolution_id INTO old_resolution FROM geo_resolution_bindings
        WHERE subject_kind='USER_LOCATION' AND subject_id=OLD.address_id;
    IF old_resolution IS NOT NULL THEN
        -- An address update later binds and reads its replacement resolution. Match
        -- worker/reader lock order before erasure takes the previous resolution lock.
        PERFORM 1 FROM geo_resolution_provider_controls c
            JOIN geo_resolutions r USING(profile) WHERE r.resolution_id=old_resolution FOR SHARE OF c;
        PERFORM 1 FROM geo_resolutions WHERE resolution_id=old_resolution FOR NO KEY UPDATE;
        DELETE FROM geo_resolution_bindings WHERE subject_kind='USER_LOCATION' AND subject_id=OLD.address_id;
        INSERT INTO geo_resolution_changes(change_id,resolution_id,generation,change_kind,subject_kind,subject_id)
            SELECT (substr(encode(int8send((extract(epoch FROM clock_timestamp())*1000)::bigint),'hex'),5,12) || '7' || substr(replace(gen_random_uuid()::text,'-',''),14,3) || '8' || substr(replace(gen_random_uuid()::text,'-',''),18,15))::uuid,resolution_id,generation,'INVALIDATED','USER_LOCATION',OLD.address_id
            FROM geo_resolutions WHERE resolution_id=old_resolution;
        UPDATE geo_resolutions r SET
            input=NULL,evidence=NULL,erased=true,status='BLOCKED',
            last_failure=NULL,generation=least(generation,9223372036854775806)+1,
            fingerprint=encode(sha256(uuid_send(r.resolution_id)),'hex')
            WHERE r.resolution_id=old_resolution AND NOT EXISTS
                (SELECT 1 FROM geo_resolution_bindings b WHERE b.resolution_id=r.resolution_id);
        DELETE FROM geo_resolution_wakeups WHERE resolution_id=old_resolution
            AND EXISTS(SELECT 1 FROM geo_resolutions WHERE resolution_id=old_resolution AND erased);
    END IF;
    IF TG_OP='DELETE' OR NEW.deleted THEN
        UPDATE user_browsing_destinations SET address_id=NULL,version=least(version,9223372036854775806)+1
            WHERE user_id=OLD.user_id AND address_id=OLD.address_id;
    END IF;
    IF TG_OP='DELETE' THEN RETURN OLD; END IF;
    RETURN NEW;
END;
$$;
