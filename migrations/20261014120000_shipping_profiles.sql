-- Advertised terms and protected evidence are authoritative, outside ordinary CDC.
-- Only immutable, address-free shipping_changes may be used for later invalidation.
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
