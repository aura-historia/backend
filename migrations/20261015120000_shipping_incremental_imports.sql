-- Shipping is pushed by authenticated producers. Provider addresses are not shipping identity.
ALTER TABLE shipping_profiles DROP CONSTRAINT shipping_profiles_integration_pair;
ALTER TABLE shipping_profiles DROP COLUMN integration_domain;
ALTER TABLE shipping_profiles ADD CONSTRAINT shipping_profiles_integration_method_valid
 CHECK(integration_method IS NULL OR integration_method IN ('SHOPIFY','WOOCOMMERCE'));
ALTER TABLE listing_source_woocommerce_ingestion_configurations DROP COLUMN shipping_integration_domain;
-- Canonical stored V2 scope is source/provider method. Strip obsolete V1 address metadata.
UPDATE product_listings SET delivery_configuration=jsonb_set(
 jsonb_set(delivery_configuration,'{integration}',(delivery_configuration->'integration')-'domain'),
 '{schema}','"LISTING_DELIVERY_V2"'::jsonb)
 WHERE delivery_configuration->>'schema'='LISTING_DELIVERY_V1';

-- Existing source/configuration lifecycle owns import authority; no shipping connection setup.
-- Kept outside the source row to preserve Party/source/user/grant lock order on revocation.
CREATE TABLE shipping_import_authority (
    listing_source_id uuid PRIMARY KEY REFERENCES listing_sources(listing_source_id) ON DELETE CASCADE,
    revision bigint NOT NULL DEFAULT 1 CHECK(revision>0),
    changed_at timestamptz NOT NULL DEFAULT clock_timestamp()
);
INSERT INTO shipping_import_authority(listing_source_id) SELECT listing_source_id FROM listing_sources;
CREATE FUNCTION initialize_shipping_import_authority() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 INSERT INTO shipping_import_authority(listing_source_id) VALUES(NEW.listing_source_id);
 RETURN NEW;
END $$;
CREATE TRIGGER initialize_shipping_import_authority AFTER INSERT ON listing_sources FOR EACH ROW EXECUTE FUNCTION initialize_shipping_import_authority();
-- These are content-safe retry fences, not retained request bodies. Replay expires after 90 days;
-- the hashes remain so expiry/deletion cannot make an executed request reusable.
CREATE TABLE shipping_import_request_fences (
    listing_source_id uuid NOT NULL,
    producer_hash bytea NOT NULL CHECK(octet_length(producer_hash)=32),
    request_hash bytea NOT NULL CHECK (octet_length(request_hash)=32),
    payload_hash bytea NOT NULL CHECK (octet_length(payload_hash)=32),
    subject_kind text NOT NULL CHECK (subject_kind IN ('PROFILE','SOURCE_DEFAULT','LISTING')),
    subject_id uuid NOT NULL,
    replay_until timestamptz NOT NULL DEFAULT now()+interval '90 days',
    PRIMARY KEY(listing_source_id,producer_hash,request_hash)
);
-- Profile identities survive retirement, manual takeover and source deletion. Only hashes of
-- producer/object keys are retained. Mutable ownership is independent of immutable identity.
CREATE TABLE shipping_profile_import_state (
    shipping_profile_id uuid PRIMARY KEY,
    listing_source_id uuid NOT NULL,
    producer_hash bytea NOT NULL CHECK (octet_length(producer_hash)=32),
    object_hash bytea NOT NULL CHECK (octet_length(object_hash)=32),
    owner_hash bytea CHECK(owner_hash IS NULL OR octet_length(owner_hash)=32),
    source_revision bigint NOT NULL,
    sequence bigint NOT NULL CHECK (sequence>0),
    observation_revision bigint NOT NULL DEFAULT 0 CHECK (observation_revision>=0),
    UNIQUE(listing_source_id,producer_hash,object_hash),
    FOREIGN KEY(listing_source_id,shipping_profile_id) REFERENCES shipping_profiles(listing_source_id,shipping_profile_id)
);
CREATE TABLE shipping_default_import_state (
    listing_source_id uuid PRIMARY KEY,
    owner_hash bytea CHECK(owner_hash IS NULL OR octet_length(owner_hash)=32),
    source_revision bigint NOT NULL,
    sequence bigint NOT NULL CHECK(sequence>0),
    observation_revision bigint NOT NULL DEFAULT 0 CHECK(observation_revision>=0),
    verified_at timestamptz,
    fresh_until timestamptz,
    expires_at timestamptz,
    CHECK ((verified_at IS NULL AND fresh_until IS NULL AND expires_at IS NULL) OR
        (verified_at IS NOT NULL AND fresh_until IS NOT NULL AND expires_at IS NOT NULL AND
         verified_at<=fresh_until AND fresh_until<=expires_at AND expires_at<=verified_at+interval '30 days'))
);
-- Listing components have independent ownership and observation validity. An explicit manual
-- clear retains a protected row; absent rows are claimable only if no assertion already exists.
CREATE TABLE listing_delivery_import_state (
    product_listing_id uuid NOT NULL REFERENCES product_listings(product_listing_id) ON DELETE CASCADE,
    component text NOT NULL CHECK(component IN ('SELECTION','RATE_INPUTS','SHIPPING','COLLECTION')),
    owner_hash bytea CHECK(owner_hash IS NULL OR octet_length(owner_hash)=32),
    source_revision bigint NOT NULL,
    sequence bigint NOT NULL CHECK(sequence>0),
    observation_revision bigint NOT NULL DEFAULT 0 CHECK(observation_revision>=0),
    verified_at timestamptz,
    fresh_until timestamptz,
    expires_at timestamptz,
    CHECK ((verified_at IS NULL AND fresh_until IS NULL AND expires_at IS NULL) OR
        (verified_at IS NOT NULL AND fresh_until IS NOT NULL AND expires_at IS NOT NULL AND
         verified_at<=fresh_until AND fresh_until<=expires_at AND expires_at<=verified_at+interval '30 days')),
    PRIMARY KEY(product_listing_id,component)
);
CREATE TABLE shipping_validity_changes (
    listing_source_id uuid NOT NULL,
    subject_kind text NOT NULL CHECK(subject_kind IN ('PROFILE','SOURCE_DEFAULT','LISTING','SOURCE')),
    subject_id uuid NOT NULL,
    revision bigint NOT NULL CHECK(revision>0),
    change_kind text NOT NULL CHECK(change_kind IN ('OBSERVED','OWNERSHIP_CHANGED','AUTHORITY_CHANGED')),
    recorded timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY(subject_kind,subject_id,revision)
);
CREATE INDEX shipping_validity_changes_source_idx ON shipping_validity_changes(listing_source_id,recorded);
CREATE TRIGGER shipping_validity_changes_immutable BEFORE UPDATE OR DELETE ON shipping_validity_changes
    FOR EACH ROW EXECUTE FUNCTION reject_shipping_history_mutation();

-- Reads and writes use the source's current lifecycle revision. Revocation advances it
-- even if a grant/configuration is later restored to the same value.
CREATE FUNCTION shipping_import_current(source_id uuid, source_revision bigint) RETURNS boolean
LANGUAGE sql STABLE AS $$
 SELECT EXISTS(SELECT 1 FROM shipping_import_authority WHERE listing_source_id=source_id AND revision=source_revision);
$$;
CREATE FUNCTION fence_shipping_imports(source_id uuid) RETURNS void LANGUAGE plpgsql AS $$
DECLARE changed_revision bigint;
BEGIN
 UPDATE shipping_import_authority SET revision=revision+1,
  changed_at=GREATEST(clock_timestamp(),changed_at+interval '1 microsecond')
 WHERE listing_source_id=source_id AND EXISTS(SELECT 1 FROM listing_sources s WHERE s.listing_source_id=source_id)
 RETURNING revision INTO changed_revision;
 IF changed_revision IS NOT NULL THEN
  INSERT INTO shipping_validity_changes VALUES(source_id,'SOURCE',source_id,changed_revision,'AUTHORITY_CHANGED',now());
 END IF;
END $$;
CREATE FUNCTION shipping_import_source_fence() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='UPDATE' AND TG_TABLE_NAME<>'listing_sources' THEN
  IF NEW IS NOT DISTINCT FROM OLD THEN RETURN NEW; END IF;
 END IF;
 IF TG_OP='DELETE' THEN
  IF TG_TABLE_NAME='listing_sources' THEN
   INSERT INTO shipping_validity_changes SELECT OLD.listing_source_id,'SOURCE',OLD.listing_source_id,revision+1,'AUTHORITY_CHANGED',now() FROM shipping_import_authority WHERE listing_source_id=OLD.listing_source_id;
  ELSE
   PERFORM fence_shipping_imports(OLD.listing_source_id);
  END IF;
  RETURN OLD;
 END IF;
 PERFORM fence_shipping_imports(NEW.listing_source_id);
 RETURN NEW;
END $$;
CREATE TRIGGER shipping_import_source_deleted BEFORE DELETE ON listing_sources FOR EACH ROW EXECUTE FUNCTION shipping_import_source_fence();
CREATE TRIGGER shipping_import_source_rebound AFTER UPDATE OF operator_party_id ON listing_sources FOR EACH ROW WHEN(OLD.operator_party_id IS DISTINCT FROM NEW.operator_party_id) EXECUTE FUNCTION shipping_import_source_fence();
CREATE TRIGGER shipping_import_method_changed AFTER INSERT OR DELETE ON listing_source_ingestion_methods FOR EACH ROW EXECUTE FUNCTION shipping_import_source_fence();
CREATE TRIGGER shipping_import_shopify_rebound AFTER INSERT OR DELETE OR UPDATE ON listing_source_shopify_ingestion_configurations FOR EACH ROW EXECUTE FUNCTION shipping_import_source_fence();
CREATE TRIGGER shipping_import_woocommerce_rebound AFTER INSERT OR DELETE OR UPDATE ON listing_source_woocommerce_ingestion_configurations FOR EACH ROW EXECUTE FUNCTION shipping_import_source_fence();
CREATE TRIGGER shipping_import_grant_revoked BEFORE DELETE ON partnership_listing_source_grants FOR EACH ROW EXECUTE FUNCTION shipping_import_source_fence();
CREATE FUNCTION shipping_import_member_fence() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE source_id uuid;
BEGIN
 IF TG_TABLE_NAME='partnership_members' THEN
  FOR source_id IN SELECT listing_source_id FROM partnership_listing_source_grants WHERE partnership_id=OLD.partnership_id LOOP
   PERFORM fence_shipping_imports(source_id);
  END LOOP;
 ELSE
  IF TG_OP='UPDATE' THEN
   IF NEW.suspended IS NOT DISTINCT FROM OLD.suspended AND NEW.role IS NOT DISTINCT FROM OLD.role THEN RETURN NEW; END IF;
  END IF;
  FOR source_id IN SELECT s.listing_source_id FROM listing_sources s WHERE OLD.role='ADMIN' OR EXISTS(
   SELECT 1 FROM partnership_members m JOIN partnership_listing_source_grants g USING(partnership_id)
   WHERE m.user_id=OLD.user_id AND g.listing_source_id=s.listing_source_id) LOOP
   PERFORM fence_shipping_imports(source_id);
  END LOOP;
 END IF;
 IF TG_OP='DELETE' THEN RETURN OLD; END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER shipping_import_membership_revoked BEFORE DELETE ON partnership_members FOR EACH ROW EXECUTE FUNCTION shipping_import_member_fence();
CREATE TRIGGER shipping_import_user_changed BEFORE DELETE OR UPDATE OF suspended,role ON users FOR EACH ROW EXECUTE FUNCTION shipping_import_member_fence();
CREATE FUNCTION shipping_import_partnership_fence() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE source_id uuid;
BEGIN
 IF NEW.business_state IS NOT DISTINCT FROM OLD.business_state AND NEW.party_id IS NOT DISTINCT FROM OLD.party_id THEN RETURN NEW; END IF;
 FOR source_id IN SELECT listing_source_id FROM partnership_listing_source_grants WHERE partnership_id=OLD.partnership_id LOOP
  PERFORM fence_shipping_imports(source_id);
 END LOOP;
 RETURN NEW;
END $$;
CREATE TRIGGER shipping_import_partnership_changed BEFORE UPDATE OF business_state,party_id ON partnerships FOR EACH ROW EXECUTE FUNCTION shipping_import_partnership_fence();

-- Ordinary writes protect changed components immediately, including a human edit after a
-- producer read. Import completion explicitly stamps its previously checked ownership.
CREATE FUNCTION shipping_profile_manual_ownership() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.management_revision IS DISTINCT FROM OLD.management_revision THEN
  UPDATE shipping_profile_import_state SET owner_hash=NULL WHERE shipping_profile_id=NEW.shipping_profile_id;
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER shipping_profile_manual_ownership AFTER UPDATE ON shipping_profiles FOR EACH ROW EXECUTE FUNCTION shipping_profile_manual_ownership();
CREATE FUNCTION shipping_default_manual_ownership() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP = 'INSERT' OR NEW.revision IS DISTINCT FROM OLD.revision THEN
  UPDATE shipping_default_import_state SET owner_hash=NULL,verified_at=NULL,fresh_until=NULL,expires_at=NULL WHERE listing_source_id=NEW.listing_source_id;
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER shipping_default_manual_ownership AFTER INSERT OR UPDATE ON shipping_source_defaults FOR EACH ROW EXECUTE FUNCTION shipping_default_manual_ownership();
CREATE FUNCTION listing_delivery_manual_ownership() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE component_name text; old_part jsonb; new_part jsonb;
BEGIN
 IF NEW.delivery_configuration IS NOT DISTINCT FROM OLD.delivery_configuration THEN RETURN NEW; END IF;
 FOREACH component_name IN ARRAY ARRAY['SELECTION','RATE_INPUTS','SHIPPING','COLLECTION'] LOOP
  old_part := CASE component_name WHEN 'SELECTION' THEN OLD.delivery_configuration->'selection' WHEN 'RATE_INPUTS' THEN OLD.delivery_configuration->'rate_inputs' WHEN 'SHIPPING' THEN OLD.delivery_configuration->'shipping' ELSE OLD.delivery_configuration->'collection' END;
  new_part := CASE component_name WHEN 'SELECTION' THEN NEW.delivery_configuration->'selection' WHEN 'RATE_INPUTS' THEN NEW.delivery_configuration->'rate_inputs' WHEN 'SHIPPING' THEN NEW.delivery_configuration->'shipping' ELSE NEW.delivery_configuration->'collection' END;
  old_part := NULLIF(old_part,'null'::jsonb);
  new_part := NULLIF(new_part,'null'::jsonb);
  -- Codec defaults do not constitute manual assertions of untouched components.
  IF component_name='RATE_INPUTS' THEN
   old_part := NULLIF(old_part,'{"classes":null,"variants":null}'::jsonb);
   new_part := NULLIF(new_part,'{"classes":null,"variants":null}'::jsonb);
  ELSIF component_name IN ('SHIPPING','COLLECTION') THEN
   old_part := NULLIF(old_part,'{"kind":"INHERIT"}'::jsonb);
   new_part := NULLIF(new_part,'{"kind":"INHERIT"}'::jsonb);
  ELSIF component_name='SELECTION' THEN
   IF OLD.delivery_configuration#>>'{integration,kind}'='SOURCE' AND old_part->>'kind'='INHERIT_SOURCE_DEFAULT' THEN old_part := NULL; END IF;
   IF NEW.delivery_configuration#>>'{integration,kind}'='SOURCE' AND new_part->>'kind'='INHERIT_SOURCE_DEFAULT' THEN new_part := NULL; END IF;
  END IF;
  IF old_part IS DISTINCT FROM new_part OR
     ((old_part IS NOT NULL OR new_part IS NOT NULL) AND OLD.delivery_configuration->'integration' IS DISTINCT FROM NEW.delivery_configuration->'integration') THEN
   INSERT INTO listing_delivery_import_state(product_listing_id,component,source_revision,sequence)
    VALUES(NEW.product_listing_id,component_name,0,1)
    ON CONFLICT(product_listing_id,component) DO UPDATE SET owner_hash=NULL,verified_at=NULL,fresh_until=NULL,expires_at=NULL;
  END IF;
 END LOOP;
 RETURN NEW;
END $$;
CREATE TRIGGER listing_delivery_manual_ownership AFTER UPDATE ON product_listings FOR EACH ROW EXECUTE FUNCTION listing_delivery_manual_ownership();
