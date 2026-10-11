-- Private data. These tables MUST NOT be selected for ordinary CDC or public indexes.
CREATE TABLE user_addresses (
    address_id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    label text,
    geography jsonb,
    input_revision bigint NOT NULL DEFAULT 1 CHECK (input_revision > 0),
    version bigint NOT NULL DEFAULT 1 CHECK (version > 0),
    deleted boolean NOT NULL DEFAULT false,
    CHECK (input_revision <= version),
    UNIQUE(user_id,address_id),
    CHECK ((deleted AND label IS NULL AND geography IS NULL) OR
           (NOT deleted AND label IS NOT NULL AND octet_length(label) BETWEEN 1 AND 128 AND geography IS NOT NULL))
);
CREATE INDEX user_addresses_page_idx ON user_addresses(user_id,address_id) WHERE NOT deleted;
CREATE TABLE user_browsing_destinations (
    user_id uuid PRIMARY KEY REFERENCES users(user_id) ON DELETE CASCADE,
    address_id uuid,
    country text,
    postal_code text,
    version bigint NOT NULL DEFAULT 1 CHECK (version > 0),
    FOREIGN KEY(user_id,address_id) REFERENCES user_addresses(user_id,address_id),
    CHECK (country IS NULL OR country ~ '^[A-Z]{2}$'),
    CHECK (postal_code IS NULL OR (country IS NOT NULL AND octet_length(postal_code) BETWEEN 1 AND 64)),
    CHECK (address_id IS NULL OR (country IS NULL AND postal_code IS NULL))
);

-- Erasure retains a content-free worker fence and the current lease reservation.
-- No raw input or provider derivative survives deletion of the last private binding.
ALTER TABLE geo_resolutions ADD COLUMN erased boolean NOT NULL DEFAULT false;
ALTER TABLE geo_resolutions ALTER COLUMN input DROP NOT NULL;
ALTER TABLE geo_resolutions ADD CHECK ((erased AND input IS NULL AND evidence IS NULL AND status='BLOCKED') OR (NOT erased AND input IS NOT NULL));

CREATE FUNCTION erase_private_address_resolution() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    old_resolution uuid;
BEGIN
    IF TG_OP='UPDATE' AND NEW.geography IS NOT DISTINCT FROM OLD.geography AND NOT NEW.deleted THEN
        RETURN NEW;
    END IF;
    SELECT resolution_id INTO old_resolution FROM geo_resolution_bindings
        WHERE subject_kind='USER_LOCATION' AND subject_id=OLD.address_id;
    IF old_resolution IS NOT NULL THEN
        PERFORM 1 FROM geo_resolutions WHERE resolution_id=old_resolution FOR NO KEY UPDATE;
        DELETE FROM geo_resolution_bindings WHERE subject_kind='USER_LOCATION' AND subject_id=OLD.address_id;
        INSERT INTO geo_resolution_changes(change_id,resolution_id,generation,change_kind,subject_kind,subject_id)
            SELECT (substr(encode(int8send((extract(epoch FROM clock_timestamp())*1000)::bigint),'hex'),5,12) || '7' || substr(replace(gen_random_uuid()::text,'-',''),14,3) || '8' || substr(replace(gen_random_uuid()::text,'-',''),18,15))::uuid,resolution_id,generation,'INVALIDATED','USER_LOCATION',OLD.address_id
            FROM geo_resolutions WHERE resolution_id=old_resolution;
        UPDATE geo_resolutions r SET
            input=NULL,evidence=NULL,valid_until=NULL,erased=true,status='BLOCKED',
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
-- Before deletion, preferences lose their composite FK and private work is detached.
CREATE TRIGGER user_address_erasure BEFORE DELETE OR UPDATE OF geography,deleted ON user_addresses
    FOR EACH ROW EXECUTE FUNCTION erase_private_address_resolution();

CREATE FUNCTION guard_user_address_identity() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.address_id<>OLD.address_id OR NEW.user_id<>OLD.user_id OR (OLD.deleted AND NOT NEW.deleted) THEN
        RAISE EXCEPTION 'user address identity is immutable' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER user_address_identity BEFORE UPDATE ON user_addresses
    FOR EACH ROW EXECUTE FUNCTION guard_user_address_identity();
