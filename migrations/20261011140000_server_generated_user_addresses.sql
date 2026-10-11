-- Server-generated address identity is separate from private creation retry metadata.
-- Legacy entries retain their IDs; retry keys cannot be inferred for old requests.
ALTER TABLE user_addresses
    ADD COLUMN create_key_hash text,
    ADD COLUMN create_fingerprint text,
    ADD CONSTRAINT user_addresses_creation_receipt_check CHECK (
        (create_key_hash IS NULL AND create_fingerprint IS NULL) OR
        (create_key_hash IS NOT NULL AND create_key_hash ~ '^[0-9a-f]{64}$' AND
            ((deleted AND create_fingerprint IS NULL) OR
             (NOT deleted AND create_fingerprint IS NOT NULL AND create_fingerprint ~ '^[0-9a-f]{64}$')))
    );
CREATE UNIQUE INDEX user_addresses_creation_key_idx ON user_addresses(user_id,create_key_hash)
    WHERE create_key_hash IS NOT NULL;

-- Keep only the key fence on deletion; erase the private input fingerprint too.
CREATE FUNCTION erase_user_address_creation_fingerprint() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.deleted THEN NEW.create_fingerprint=NULL; END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER user_address_creation_erasure BEFORE UPDATE OF deleted ON user_addresses
    FOR EACH ROW EXECUTE FUNCTION erase_user_address_creation_fingerprint();

CREATE OR REPLACE FUNCTION guard_user_address_identity() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.address_id<>OLD.address_id OR NEW.user_id<>OLD.user_id OR (OLD.deleted AND NOT NEW.deleted)
        OR NEW.create_key_hash IS DISTINCT FROM OLD.create_key_hash
        OR (NEW.create_fingerprint IS DISTINCT FROM OLD.create_fingerprint
            AND NOT (NEW.deleted AND NEW.create_fingerprint IS NULL)) THEN
        RAISE EXCEPTION 'user address identity is immutable' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
