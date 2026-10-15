-- Per-source acquisition epochs fence disconnected/replaced producers. There are no upload sessions.
CREATE TABLE shipping_import_connections (
    listing_source_id uuid PRIMARY KEY,
    revision bigint NOT NULL CHECK (revision > 0),
    actor text NOT NULL,
    user_id uuid,
    integration_method text,
    integration_domain text,
    active boolean NOT NULL,
    connected_at timestamptz NOT NULL,
    CHECK ((actor LIKE 'user:%') = (user_id IS NOT NULL)),
    CHECK ((integration_method IS NULL AND integration_domain IS NULL) OR
        (integration_method IN ('SHOPIFY','WOOCOMMERCE') AND integration_domain IS NOT NULL))
);
-- These are content-safe retry fences, not retained request bodies. Replay expires after 90 days;
-- the hashes remain so expiry/deletion cannot make an executed request reusable.
CREATE TABLE shipping_import_request_fences (
    listing_source_id uuid NOT NULL,
    connection_revision bigint NOT NULL,
    request_hash bytea NOT NULL CHECK (octet_length(request_hash)=32),
    payload_hash bytea NOT NULL CHECK (octet_length(payload_hash)=32),
    subject_kind text NOT NULL CHECK (subject_kind IN ('PROFILE','SOURCE_DEFAULT','LISTING')),
    subject_id uuid NOT NULL,
    replay_until timestamptz NOT NULL DEFAULT now()+interval '90 days',
    PRIMARY KEY(listing_source_id,connection_revision,request_hash)
);
-- Profile identities survive retirement, manual takeover and source deletion. Only hashes of
-- producer/object keys are retained. Mutable ownership is independent of immutable identity.
CREATE TABLE shipping_profile_import_state (
    shipping_profile_id uuid PRIMARY KEY,
    listing_source_id uuid NOT NULL,
    producer_hash bytea NOT NULL CHECK (octet_length(producer_hash)=32),
    object_hash bytea NOT NULL CHECK (octet_length(object_hash)=32),
    owner_hash bytea CHECK(owner_hash IS NULL OR octet_length(owner_hash)=32),
    connection_revision bigint NOT NULL,
    sequence bigint NOT NULL CHECK (sequence>0),
    observation_revision bigint NOT NULL DEFAULT 0 CHECK (observation_revision>=0),
    UNIQUE(listing_source_id,producer_hash,object_hash),
    FOREIGN KEY(listing_source_id,shipping_profile_id) REFERENCES shipping_profiles(listing_source_id,shipping_profile_id)
);
CREATE TABLE shipping_default_import_state (
    listing_source_id uuid PRIMARY KEY,
    owner_hash bytea CHECK(owner_hash IS NULL OR octet_length(owner_hash)=32),
    connection_revision bigint NOT NULL,
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
    connection_revision bigint NOT NULL,
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
    subject_kind text NOT NULL CHECK(subject_kind IN ('PROFILE','SOURCE_DEFAULT','LISTING','CONNECTION')),
    subject_id uuid NOT NULL,
    revision bigint NOT NULL CHECK(revision>0),
    change_kind text NOT NULL CHECK(change_kind IN ('OBSERVED','OWNERSHIP_CHANGED','CONNECTION_CHANGED')),
    recorded timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY(subject_kind,subject_id,revision)
);
CREATE INDEX shipping_validity_changes_source_idx ON shipping_validity_changes(listing_source_id,recorded);
CREATE TRIGGER shipping_validity_changes_immutable BEFORE UPDATE OR DELETE ON shipping_validity_changes
    FOR EACH ROW EXECUTE FUNCTION reject_shipping_history_mutation();

-- Eligibility is checked in the same authoritative snapshot as advertised terms. Current
-- authorization also gates application/replay. Revocation triggers below prevent regrant ABA.
CREATE FUNCTION shipping_import_connection_current(source_id uuid, connection_revision bigint) RETURNS boolean
LANGUAGE sql STABLE AS $$
SELECT EXISTS(
 SELECT 1 FROM shipping_import_connections c JOIN listing_sources s USING(listing_source_id)
 WHERE c.listing_source_id=source_id AND c.revision=connection_revision AND c.active
 AND (c.user_id IS NULL OR EXISTS(SELECT 1 FROM users u WHERE u.user_id=c.user_id AND NOT u.suspended
      AND (u.role='ADMIN' OR EXISTS(SELECT 1 FROM partnership_members m JOIN partnerships p USING(partnership_id)
          JOIN partnership_listing_source_grants g USING(partnership_id)
          WHERE m.user_id=u.user_id AND p.party_id=s.operator_party_id AND p.business_state='ACTIVE' AND g.listing_source_id=source_id))))
 AND ((c.integration_method IS NULL AND NOT EXISTS(SELECT 1 FROM listing_source_ingestion_methods m WHERE m.listing_source_id=source_id AND m.ingestion_method IN ('SHOPIFY','WOOCOMMERCE')))
 OR (EXISTS(SELECT 1 FROM listing_source_ingestion_methods m WHERE m.listing_source_id=source_id AND m.ingestion_method=c.integration_method)
  AND ((c.integration_method='SHOPIFY' AND EXISTS(SELECT 1 FROM listing_source_shopify_ingestion_configurations sh WHERE sh.listing_source_id=source_id AND sh.domain=c.integration_domain))
   OR (c.integration_method='WOOCOMMERCE' AND EXISTS(SELECT 1 FROM listing_source_woocommerce_ingestion_configurations w WHERE w.listing_source_id=source_id AND w.shipping_integration_domain=c.integration_domain)))))
);
$$;
CREATE FUNCTION fence_shipping_import_connection(source_id uuid) RETURNS void LANGUAGE plpgsql AS $$
DECLARE changed_revision bigint;
BEGIN
 UPDATE shipping_import_connections SET active=false,revision=revision+1 WHERE listing_source_id=source_id AND active RETURNING revision INTO changed_revision;
 IF changed_revision IS NOT NULL THEN
  INSERT INTO shipping_validity_changes VALUES(source_id,'CONNECTION',source_id,changed_revision,'CONNECTION_CHANGED',now());
 END IF;
END $$;
CREATE FUNCTION shipping_import_source_fence() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='UPDATE' THEN
  IF TG_TABLE_NAME='listing_source_shopify_ingestion_configurations' THEN
   IF NEW.domain IS NOT DISTINCT FROM OLD.domain THEN RETURN NEW; END IF;
  ELSIF TG_TABLE_NAME='listing_source_woocommerce_ingestion_configurations' THEN
   IF NEW.shipping_integration_domain IS NOT DISTINCT FROM OLD.shipping_integration_domain
      AND NEW.webhook_secret IS NOT DISTINCT FROM OLD.webhook_secret THEN RETURN NEW; END IF;
  END IF;
 END IF;
 PERFORM fence_shipping_import_connection(OLD.listing_source_id);
 IF TG_OP='DELETE' THEN RETURN OLD; END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER shipping_import_source_deleted BEFORE DELETE ON listing_sources FOR EACH ROW EXECUTE FUNCTION shipping_import_source_fence();
CREATE TRIGGER shipping_import_source_rebound AFTER UPDATE OF operator_party_id ON listing_sources FOR EACH ROW WHEN(OLD.operator_party_id IS DISTINCT FROM NEW.operator_party_id) EXECUTE FUNCTION shipping_import_source_fence();
CREATE TRIGGER shipping_import_method_removed BEFORE DELETE ON listing_source_ingestion_methods FOR EACH ROW EXECUTE FUNCTION shipping_import_source_fence();
CREATE TRIGGER shipping_import_shopify_rebound BEFORE DELETE OR UPDATE OF domain ON listing_source_shopify_ingestion_configurations FOR EACH ROW EXECUTE FUNCTION shipping_import_source_fence();
CREATE TRIGGER shipping_import_woocommerce_rebound BEFORE DELETE OR UPDATE OF shipping_integration_domain,webhook_secret ON listing_source_woocommerce_ingestion_configurations FOR EACH ROW EXECUTE FUNCTION shipping_import_source_fence();
CREATE TRIGGER shipping_import_grant_revoked BEFORE DELETE ON partnership_listing_source_grants FOR EACH ROW EXECUTE FUNCTION shipping_import_source_fence();
CREATE FUNCTION shipping_import_member_fence() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE source_id uuid;
BEGIN
 IF TG_OP='UPDATE' THEN
  IF NEW.suspended IS NOT DISTINCT FROM OLD.suspended AND NEW.role IS NOT DISTINCT FROM OLD.role THEN RETURN NEW; END IF;
 END IF;
 FOR source_id IN SELECT c.listing_source_id FROM shipping_import_connections c WHERE c.user_id=OLD.user_id LOOP
  PERFORM fence_shipping_import_connection(source_id);
 END LOOP;
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
  PERFORM fence_shipping_import_connection(source_id);
 END LOOP;
 IF TG_OP='DELETE' THEN RETURN OLD; END IF;
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
   INSERT INTO listing_delivery_import_state(product_listing_id,component,connection_revision,sequence)
    VALUES(NEW.product_listing_id,component_name,0,1)
    ON CONFLICT(product_listing_id,component) DO UPDATE SET owner_hash=NULL,verified_at=NULL,fresh_until=NULL,expires_at=NULL;
  END IF;
 END LOOP;
 RETURN NEW;
END $$;
CREATE TRIGGER listing_delivery_manual_ownership AFTER UPDATE ON product_listings FOR EACH ROW EXECUTE FUNCTION listing_delivery_manual_ownership();
