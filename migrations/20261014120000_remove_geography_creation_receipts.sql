-- Contract migration: deploy the keyless address/location runtimes and drain old
-- invocations before applying. The new runtimes also work with the previous schema;
-- receipt-dependent binaries are not compatible with this schema after cleanup.
DROP TABLE party_location_create_receipts;

-- Identity and deletion guards remain independent of creation deduplication.
CREATE OR REPLACE FUNCTION guard_user_address_identity() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.address_id<>OLD.address_id OR NEW.user_id<>OLD.user_id OR (OLD.deleted AND NOT NEW.deleted) THEN
        RAISE EXCEPTION 'user address identity is immutable' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;

DROP TRIGGER user_address_creation_erasure ON user_addresses;
DROP FUNCTION erase_user_address_creation_fingerprint();
DROP INDEX user_addresses_creation_key_idx;
ALTER TABLE user_addresses
    DROP CONSTRAINT user_addresses_creation_receipt_check,
    DROP COLUMN create_key_hash,
    DROP COLUMN create_fingerprint;

-- Keep address ID tombstones, private-content erasure hooks and geography worker
-- fences so delayed work cannot restore deleted private content.
