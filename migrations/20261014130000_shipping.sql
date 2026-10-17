-- Advertised terms and protected evidence are authoritative, outside ordinary CDC.
-- Immutable, address-free change signals drive bounded projection invalidation.
CREATE TABLE shipping_profiles (
    shipping_profile_id uuid PRIMARY KEY,
    listing_source_id uuid NOT NULL,
    current_source_id uuid REFERENCES listing_sources(listing_source_id) ON DELETE SET NULL,
    management_revision bigint NOT NULL CHECK (management_revision > 0),
    terms_revision bigint NOT NULL CHECK (terms_revision > 0 AND terms_revision <= management_revision),
    lifecycle text NOT NULL CHECK (lifecycle IN ('ACTIVE','RETIRED')),
    name text NOT NULL CHECK (octet_length(name) BETWEEN 1 AND 255),
    attributed_party_id uuid REFERENCES parties(party_id),
    terms jsonb NOT NULL CHECK (jsonb_typeof(terms) = 'object'),
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    integration_method text CONSTRAINT shipping_profiles_integration_method_valid
        CHECK (integration_method IS NULL OR integration_method IN ('SHOPIFY','WOOCOMMERCE')),
    verified_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (listing_source_id, shipping_profile_id),
    CHECK (current_source_id IS NULL OR current_source_id = listing_source_id)
);
CREATE INDEX shipping_profiles_source_idx ON shipping_profiles(listing_source_id, shipping_profile_id);

CREATE TABLE shipping_profile_evidence (
    shipping_profile_id uuid NOT NULL REFERENCES shipping_profiles(shipping_profile_id),
    management_revision bigint NOT NULL CHECK (management_revision > 0),
    evidence jsonb NOT NULL CHECK (jsonb_typeof(evidence) = 'array'),
    actor text NOT NULL,
    recorded timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (shipping_profile_id, management_revision)
);
CREATE TABLE shipping_profile_history (
    shipping_profile_id uuid NOT NULL REFERENCES shipping_profiles(shipping_profile_id),
    management_revision bigint NOT NULL CHECK (management_revision > 0),
    terms_revision bigint NOT NULL CHECK (terms_revision > 0 AND terms_revision <= management_revision),
    lifecycle text NOT NULL CHECK (lifecycle IN ('ACTIVE','RETIRED')),
    name text NOT NULL,
    attributed_party_id uuid,
    terms jsonb NOT NULL,
    PRIMARY KEY (shipping_profile_id, management_revision)
);
-- Immutable original creation semantics survive edits, retirement and source deletion.
CREATE TABLE shipping_profile_creation_receipts (
    listing_source_id uuid NOT NULL,
    caller text NOT NULL,
    creation_key text NOT NULL CHECK (octet_length(creation_key) BETWEEN 1 AND 128),
    shipping_profile_id uuid NOT NULL REFERENCES shipping_profiles(shipping_profile_id),
    PRIMARY KEY (listing_source_id, caller, creation_key)
);
CREATE TABLE shipping_source_defaults (
    listing_source_id uuid PRIMARY KEY,
    current_source_id uuid REFERENCES listing_sources(listing_source_id) ON DELETE SET NULL,
    shipping_profile_id uuid,
    revision bigint NOT NULL CHECK (revision >= 2),
    source_deleted boolean NOT NULL DEFAULT false,
    FOREIGN KEY (listing_source_id, shipping_profile_id) REFERENCES shipping_profiles(listing_source_id, shipping_profile_id),
    CHECK (current_source_id IS NULL OR current_source_id = listing_source_id),
    CHECK (NOT source_deleted OR shipping_profile_id IS NULL)
);
CREATE INDEX shipping_source_defaults_profile_idx ON shipping_source_defaults(shipping_profile_id, listing_source_id) WHERE shipping_profile_id IS NOT NULL;

-- Reverse references for bounded location invalidation; no hydrated addresses or inferred applicability.
CREATE TABLE shipping_profile_location_references (
    shipping_profile_id uuid NOT NULL REFERENCES shipping_profiles(shipping_profile_id),
    party_location_id uuid NOT NULL REFERENCES party_locations(party_location_id),
    applicability text NOT NULL CHECK (applicability IN ('DISPATCH','COLLECTION')),
    PRIMARY KEY (shipping_profile_id, applicability)
);
CREATE INDEX shipping_profile_locations_reverse_idx ON shipping_profile_location_references(party_location_id, shipping_profile_id);

CREATE TABLE shipping_changes (
    schema_version smallint NOT NULL DEFAULT 1 CHECK (schema_version = 1),
    listing_source_id uuid NOT NULL,
    subject_kind text NOT NULL CHECK (subject_kind IN ('PROFILE','SOURCE_DEFAULT')),
    subject_id uuid NOT NULL,
    revision bigint NOT NULL CHECK (revision > 0),
    terms_revision bigint CHECK (terms_revision > 0),
    change_kind text NOT NULL CHECK (change_kind IN ('CREATED','REPLACED','RETIRED','RESTORED','EVIDENCE_CORRECTED','DEFAULT_ASSIGNED','DEFAULT_CLEARED','SOURCE_DELETED')),
    actor text NOT NULL,
    recorded timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (subject_kind, subject_id, revision),
    CHECK ((subject_kind = 'PROFILE') = (terms_revision IS NOT NULL)),
    CHECK (subject_kind <> 'SOURCE_DEFAULT' OR subject_id = listing_source_id)
);
CREATE INDEX shipping_changes_source_idx ON shipping_changes(listing_source_id, recorded, subject_id, revision);

-- Delete fences survive the source. No dependent listing is synchronously rewritten.
CREATE FUNCTION tombstone_shipping_source() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO shipping_source_defaults(listing_source_id,current_source_id,revision,source_deleted)
    VALUES(OLD.listing_source_id,OLD.listing_source_id,2,true)
    ON CONFLICT (listing_source_id) DO UPDATE SET shipping_profile_id=NULL,
        revision=shipping_source_defaults.revision+1,source_deleted=true;
    INSERT INTO shipping_changes(listing_source_id,subject_kind,subject_id,revision,change_kind,actor)
    SELECT listing_source_id,'SOURCE_DEFAULT',listing_source_id,revision,'SOURCE_DELETED','system:source-deletion'
    FROM shipping_source_defaults WHERE listing_source_id=OLD.listing_source_id;
    RETURN OLD;
END $$;
CREATE TRIGGER shipping_source_deletion BEFORE DELETE ON listing_sources FOR EACH ROW EXECUTE FUNCTION tombstone_shipping_source();

CREATE FUNCTION reject_shipping_history_mutation() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'shipping history and change signals are immutable';
END $$;
CREATE TRIGGER shipping_changes_immutable BEFORE UPDATE OR DELETE ON shipping_changes FOR EACH ROW EXECUTE FUNCTION reject_shipping_history_mutation();
CREATE TRIGGER shipping_history_immutable BEFORE UPDATE OR DELETE ON shipping_profile_history FOR EACH ROW EXECUTE FUNCTION reject_shipping_history_mutation();
CREATE TRIGGER shipping_evidence_immutable BEFORE UPDATE OR DELETE ON shipping_profile_evidence FOR EACH ROW EXECUTE FUNCTION reject_shipping_history_mutation();
CREATE TRIGGER shipping_receipts_immutable BEFORE UPDATE OR DELETE ON shipping_profile_creation_receipts FOR EACH ROW EXECUTE FUNCTION reject_shipping_history_mutation();

-- Delivery is part of the listing revision/transaction, never an independently mutable row.
ALTER TABLE product_listings ADD COLUMN delivery_configuration jsonb,
    ADD COLUMN delivery_verified_at timestamptz;
ALTER TABLE product_listings ADD CONSTRAINT product_listings_delivery_object
    CHECK (delivery_configuration IS NULL OR jsonb_typeof(delivery_configuration) = 'object'),
    ADD CONSTRAINT listing_delivery_evidence_window CHECK (
        (delivery_configuration IS NULL) = (delivery_verified_at IS NULL));
CREATE INDEX product_listings_delivery_profile_idx
    ON product_listings ((delivery_configuration #>> '{selection,profile_id}'))
    WHERE delivery_configuration #>> '{selection,kind}' = 'EXPLICIT';
CREATE INDEX product_listings_delivery_source_idx ON product_listings (listing_source_id)
    WHERE delivery_configuration IS NOT NULL;
ALTER TABLE product_listing_command_receipts DROP CONSTRAINT product_listing_command_receipts_operation_check;
ALTER TABLE product_listing_command_receipts ADD CONSTRAINT product_listing_command_receipts_operation_check
    CHECK (operation IN ('CREATE','UPDATE','UPSERT','WITHDRAW','CAPTURE_RAW','CHANGE_DELIVERY'));

CREATE FUNCTION shipping_assertion_verification() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.terms IS DISTINCT FROM OLD.terms THEN
        NEW.verified_at := now();
    END IF;
    RETURN NEW;
END $$;

CREATE FUNCTION listing_delivery_verification() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        IF NEW.delivery_configuration IS NOT NULL THEN NEW.delivery_verified_at := now(); END IF;
    ELSIF NEW.delivery_configuration IS DISTINCT FROM OLD.delivery_configuration THEN
        NEW.delivery_verified_at := CASE WHEN NEW.delivery_configuration IS NOT NULL THEN now() END;
    END IF;
    RETURN NEW;
END $$;

CREATE TRIGGER shipping_assertion_verification BEFORE UPDATE ON shipping_profiles
    FOR EACH ROW EXECUTE FUNCTION shipping_assertion_verification();
CREATE TRIGGER listing_delivery_verification BEFORE INSERT OR UPDATE ON product_listings
    FOR EACH ROW EXECUTE FUNCTION listing_delivery_verification();

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
    verified_at timestamptz
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
  UPDATE shipping_default_import_state SET owner_hash=NULL,verified_at=NULL WHERE listing_source_id=NEW.listing_source_id;
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
    ON CONFLICT(product_listing_id,component) DO UPDATE SET owner_hash=NULL,verified_at=NULL;
  END IF;
 END LOOP;
 RETURN NEW;
END $$;
CREATE TRIGGER listing_delivery_manual_ownership AFTER UPDATE ON product_listings FOR EACH ROW EXECUTE FUNCTION listing_delivery_manual_ownership();

-- Rebuildable, public-safe discovery state. Authoritative policies/observations stay private.
CREATE TABLE product_listing_shipping (
    product_listing_id uuid PRIMARY KEY REFERENCES product_listings(product_listing_id) ON DELETE CASCADE,
    dependency jsonb NOT NULL CHECK(jsonb_typeof(dependency)='object'),
    summary jsonb NOT NULL CHECK(jsonb_typeof(summary)='object'),
    projection_version bigint NOT NULL CHECK(projection_version>0)
);
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

-- Evaluation provenance is independent of the listing's sale/asking-price basis.
ALTER TABLE search_filter_matches
    ADD COLUMN shipping_fx_rate_id uuid REFERENCES fx_rates(fx_rate_id) ON DELETE RESTRICT,
    ADD COLUMN shipping_fx_captured_at timestamptz,
    ADD COLUMN shipping_target_currency text,
    ADD COLUMN shipping_maximum_minor bigint,
    ADD CONSTRAINT search_filter_matches_shipping_fx_check CHECK (
        (shipping_fx_rate_id IS NULL AND shipping_fx_captured_at IS NULL
         AND shipping_target_currency IS NULL AND shipping_maximum_minor IS NULL)
        OR
        (shipping_fx_rate_id IS NOT NULL AND shipping_fx_captured_at IS NOT NULL
         AND shipping_target_currency IS NOT NULL AND shipping_maximum_minor IS NOT NULL
         AND shipping_maximum_minor >= 0
         AND shipping_target_currency IN (
             'EUR','GBP','USD','AUD','CAD','NZD','CNY','BRL','PLN','TRY','JPY','CZK',
             'RUB','AED','SAR','HKD','SGD','CHF','ZAR','SEK','DKK','NOK','KRW','INR',
             'TWD','HUF','RON','MXN','THB'
         ))
    );

-- Initial source traversal is bounded by workers; concurrent listings have independent work.
SELECT enqueue_listing_shipping('SOURCE',listing_source_id::text) FROM listing_sources;
