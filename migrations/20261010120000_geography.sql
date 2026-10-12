-- Geographic locations, private address books, durable resolution and listing discovery.
-- First deployment: create the final schema directly.

-- Party associations with independently identified sites; no global address identity/default.
CREATE TABLE party_locations (
    party_location_id uuid PRIMARY KEY,
    party_id uuid NOT NULL REFERENCES parties(party_id) ON DELETE RESTRICT,
    label text NOT NULL CHECK (octet_length(label) BETWEEN 1 AND 255 AND length(trim(label)) > 0),
    roles text[] NOT NULL DEFAULT '{}' CHECK (roles <@ ARRAY['BUSINESS_PREMISES','WAREHOUSE','REGISTERED_ADDRESS','CORRESPONDENCE']::text[] AND cardinality(roles) <= 4),
    geography jsonb,
    position jsonb,
    lifecycle text NOT NULL CHECK (lifecycle IN ('ACTIVE','RETIRED')),
    revision bigint NOT NULL DEFAULT 1 CHECK (revision > 0),
    input_revision bigint NOT NULL DEFAULT 1 CHECK (input_revision > 0 AND input_revision <= revision),
    evidence jsonb,
    created_actor text NOT NULL,
    updated_actor text NOT NULL,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX party_locations_party_cursor_idx ON party_locations(party_id, party_location_id);

-- All access/grant changes serialize on the owner Party row. Listing scopes confer no grant.
CREATE TABLE party_location_management_grants (
    party_id uuid NOT NULL REFERENCES parties(party_id) ON DELETE RESTRICT,
    user_id uuid NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    granted_actor text NOT NULL,
    PRIMARY KEY (party_id, user_id)
);

-- Private completed-command receipts. No TTL, CDC selection or normalized-address identity.
CREATE TABLE party_location_create_receipts (
    party_id uuid NOT NULL REFERENCES parties(party_id) ON DELETE RESTRICT,
    actor text NOT NULL,
    idempotency_key text NOT NULL CHECK (octet_length(idempotency_key) BETWEEN 1 AND 128),
    command jsonb NOT NULL,
    result jsonb NOT NULL,
    PRIMARY KEY (party_id, actor, idempotency_key)
);

-- Minimal immutable change signal for later dependent-geography invalidation. No raw evidence.
CREATE TABLE party_location_changes (
    party_location_id uuid NOT NULL REFERENCES party_locations(party_location_id) ON DELETE RESTRICT,
    revision bigint NOT NULL CHECK (revision > 0),
    party_id uuid NOT NULL REFERENCES parties(party_id) ON DELETE RESTRICT,
    input_revision bigint NOT NULL CHECK (input_revision > 0),
    lifecycle text NOT NULL CHECK (lifecycle IN ('ACTIVE','RETIRED')),
    actor text NOT NULL,
    occurred_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (party_location_id, revision)
);

-- Private authoritative inputs/results. Only geo_resolution_wakeups is selected for CDC.
CREATE TABLE geo_resolution_provider_controls (
    profile text PRIMARY KEY,
    is_default boolean NOT NULL DEFAULT false,
    enabled boolean NOT NULL DEFAULT false,
    storage_permitted boolean NOT NULL DEFAULT false,
    daily_limit integer NOT NULL DEFAULT 100 CHECK (daily_limit BETWEEN 1 AND 100000),
    scope_daily_limit integer NOT NULL DEFAULT 20 CHECK (scope_daily_limit BETWEEN 1 AND 100000),
    concurrency_limit integer NOT NULL DEFAULT 2 CHECK (concurrency_limit BETWEEN 1 AND 8),
    scope_concurrency_limit integer NOT NULL DEFAULT 1 CHECK (scope_concurrency_limit BETWEEN 1 AND 8),
    budget_day date NOT NULL DEFAULT (now() AT TIME ZONE 'UTC')::date,
    calls integer NOT NULL DEFAULT 0 CHECK (calls >= 0),
    consecutive_failures integer NOT NULL DEFAULT 0 CHECK (consecutive_failures >= 0),
    open_until timestamptz
);
CREATE UNIQUE INDEX geo_resolution_default_profile_idx ON geo_resolution_provider_controls(is_default) WHERE is_default;
INSERT INTO geo_resolution_provider_controls(profile,is_default)
VALUES ('AMAZON_LOCATION_PLACES_V2_DEFAULT_FRANKFURT_2026_10',true);

CREATE TABLE geo_resolution_scope_budgets (
    profile text NOT NULL REFERENCES geo_resolution_provider_controls(profile),
    scope_key text NOT NULL CHECK (scope_key ~ '^[0-9a-f]{64}$'),
    budget_day date NOT NULL,
    calls integer NOT NULL CHECK (calls >= 0),
    PRIMARY KEY(profile,scope_key)
);
CREATE TABLE geo_resolutions (
    resolution_id uuid PRIMARY KEY,
    fingerprint text NOT NULL UNIQUE CHECK (fingerprint ~ '^[0-9a-f]{64}$'),
    profile text NOT NULL REFERENCES geo_resolution_provider_controls(profile),
    policy_version text NOT NULL CHECK (policy_version IN ('PRECISION_FIRST_V1','AUTOMATIC_USER_BROWSING_V1')),
    scope_key text NOT NULL CHECK (scope_key ~ '^[0-9a-f]{64}$'),
    input jsonb,
    erased boolean NOT NULL DEFAULT false,
    generation bigint NOT NULL DEFAULT 1 CHECK (generation > 0),
    status text NOT NULL DEFAULT 'PENDING' CHECK (status IN ('PENDING','ACCEPTED','AMBIGUOUS','NO_MATCH','FAILED','BLOCKED')),
    attempts integer NOT NULL DEFAULT 0 CHECK (attempts BETWEEN 0 AND 5),
    lease_token uuid,
    lease_until timestamptz,
    next_attempt_at timestamptz NOT NULL DEFAULT now(),
    evidence jsonb,
    valid_until timestamptz,
    last_failure text,
    created timestamptz NOT NULL DEFAULT now(),
    pending_since timestamptz NOT NULL DEFAULT now(),
    CHECK ((erased AND input IS NULL AND evidence IS NULL AND status='BLOCKED') OR (NOT erased AND input IS NOT NULL)),
    CHECK ((lease_token IS NULL) = (lease_until IS NULL)),
    CHECK ((evidence IS NULL) = (valid_until IS NULL)),
    CHECK ((status IN ('ACCEPTED','AMBIGUOUS','NO_MATCH')) = (evidence IS NOT NULL)),
    CHECK (status NOT IN ('FAILED','BLOCKED') OR evidence IS NULL)
);
CREATE INDEX geo_resolution_pending_idx ON geo_resolutions(next_attempt_at,created) WHERE status='PENDING';
CREATE INDEX geo_resolution_expiry_idx ON geo_resolutions(valid_until) WHERE evidence IS NOT NULL;

-- Bound only by owning services under their authorization/aggregate lock. Removing or
-- changing a binding immediately makes the old geography ineligible for that subject.
CREATE TABLE geo_resolution_bindings (
    subject_kind text NOT NULL CHECK (subject_kind IN ('PARTY_LOCATION','USER_LOCATION')),
    subject_id uuid NOT NULL,
    resolution_id uuid NOT NULL REFERENCES geo_resolutions(resolution_id),
    input_revision bigint NOT NULL CHECK (input_revision > 0),
    guard bigint NOT NULL CHECK (guard > 0),
    selected_candidate integer CHECK (selected_candidate BETWEEN 0 AND 10),
    selected_generation bigint CHECK (selected_generation > 0),
    reviewed_actor text,
    PRIMARY KEY(subject_kind,subject_id),
    CHECK ((selected_candidate IS NULL) = (selected_generation IS NULL)),
    CHECK ((selected_candidate IS NULL) = (reviewed_actor IS NULL))
);
CREATE INDEX geo_resolution_binding_resolution_idx ON geo_resolution_bindings(resolution_id);

-- Compact immutable wake-ups: generations fence old retries/replays.
CREATE TABLE geo_resolution_wakeups (
    resolution_id uuid NOT NULL REFERENCES geo_resolutions(resolution_id),
    generation bigint NOT NULL CHECK (generation > 0),
    PRIMARY KEY(resolution_id,generation)
);
-- Durable enrichment/invalidation signal, including binding review and expiry. It
-- contains no provider evidence and is reserved for dependent-geography consumers.
CREATE TABLE geo_resolution_changes (
    change_id uuid PRIMARY KEY,
    resolution_id uuid NOT NULL REFERENCES geo_resolutions(resolution_id),
    generation bigint NOT NULL CHECK (generation > 0),
    change_kind text NOT NULL CHECK (change_kind IN ('RESOLVED','INVALIDATED','REVIEWED','REQUEUED')),
    subject_kind text,
    subject_id uuid,
    occurred_at timestamptz NOT NULL DEFAULT now(),
    CHECK ((subject_kind IS NULL) = (subject_id IS NULL))
);

-- One explicitly selected operator-owned location per source; no copied geography.
ALTER TABLE listing_sources ADD CONSTRAINT listing_sources_operator_identity UNIQUE (listing_source_id, operator_party_id);
ALTER TABLE party_locations ADD CONSTRAINT party_locations_owner_identity UNIQUE (party_location_id, party_id);
CREATE TABLE listing_source_location_defaults (
    listing_source_id uuid PRIMARY KEY,
    party_id uuid NOT NULL,
    party_location_id uuid,
    business_evidence jsonb,
    inventory_evidence jsonb,
    revision bigint NOT NULL CHECK (revision >= 2),
    updated_actor text NOT NULL,
    updated timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (listing_source_id, party_id) REFERENCES listing_sources(listing_source_id, operator_party_id) ON DELETE CASCADE,
    FOREIGN KEY (party_location_id, party_id) REFERENCES party_locations(party_location_id, party_id) ON DELETE RESTRICT,
    CHECK (party_location_id IS NOT NULL OR (business_evidence IS NULL AND inventory_evidence IS NULL)),
    CHECK (business_evidence IS NULL OR (jsonb_typeof(business_evidence) = 'object' AND business_evidence ?& ARRAY['reference','scope'] AND jsonb_typeof(business_evidence->'scope') = 'string' AND jsonb_typeof(business_evidence->'reference') = 'string' AND business_evidence->>'scope' = 'ASSOCIATED_BUSINESS' AND business_evidence - 'reference' - 'scope' = '{}'::jsonb)),
    CHECK (inventory_evidence IS NULL OR (jsonb_typeof(inventory_evidence) = 'object' AND inventory_evidence ?& ARRAY['reference','scope'] AND jsonb_typeof(inventory_evidence->'scope') = 'string' AND jsonb_typeof(inventory_evidence->'reference') = 'string' AND inventory_evidence->>'scope' = 'ADVERTISED_INVENTORY' AND inventory_evidence - 'reference' - 'scope' = '{}'::jsonb))
);
CREATE INDEX listing_source_location_defaults_location_idx ON listing_source_location_defaults(party_location_id) WHERE party_location_id IS NOT NULL;

-- Retained immutable invalidation basis, independent of source deletion. Never evidence/addresses.
CREATE TABLE listing_source_location_default_changes (
    listing_source_id uuid NOT NULL,
    revision bigint NOT NULL CHECK (revision >= 2),
    party_id uuid NOT NULL,
    party_location_id uuid,
    location_revision bigint CHECK (location_revision > 0),
    business_association boolean NOT NULL,
    advertised_inventory boolean NOT NULL,
    actor text NOT NULL,
    occurred_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (listing_source_id, revision),
    CHECK ((party_location_id IS NULL) = (location_revision IS NULL)),
    CHECK (party_location_id IS NOT NULL OR (NOT business_association AND NOT advertised_inventory))
);

-- Private data. These tables MUST NOT be selected for ordinary CDC or public indexes.
CREATE TABLE user_addresses (
    address_id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    create_key_hash text,
    create_fingerprint text,
    CONSTRAINT user_addresses_creation_receipt_check CHECK (
        (create_key_hash IS NULL AND create_fingerprint IS NULL) OR
        (create_key_hash IS NOT NULL AND create_key_hash ~ '^[0-9a-f]{64}$' AND
            ((deleted AND create_fingerprint IS NULL) OR
             (NOT deleted AND create_fingerprint IS NOT NULL AND create_fingerprint ~ '^[0-9a-f]{64}$')))
    ),
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
    IF NEW.address_id<>OLD.address_id OR NEW.user_id<>OLD.user_id OR (OLD.deleted AND NOT NEW.deleted)
        OR NEW.create_key_hash IS DISTINCT FROM OLD.create_key_hash
        OR (NEW.create_fingerprint IS DISTINCT FROM OLD.create_fingerprint
            AND NOT (NEW.deleted AND NEW.create_fingerprint IS NULL)) THEN
        RAISE EXCEPTION 'user address identity is immutable' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER user_address_identity BEFORE UPDATE ON user_addresses
    FOR EACH ROW EXECUTE FUNCTION guard_user_address_identity();

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


-- Bounded derived read state. Addresses/evidence remain at their owning authority.
CREATE TABLE product_listing_geographies (
    product_listing_id uuid PRIMARY KEY REFERENCES product_listings(product_listing_id) ON DELETE CASCADE,
    dependency jsonb NOT NULL,
    geography jsonb NOT NULL,
    valid_until timestamptz,
    geography_version bigint NOT NULL CHECK (geography_version > 0),
    indexed_version bigint NOT NULL DEFAULT 0 CHECK (indexed_version >= 0)
);
CREATE INDEX product_listing_geographies_unindexed_idx ON product_listing_geographies(product_listing_id) WHERE indexed_version < geography_version;
CREATE INDEX product_listing_geographies_expiry_idx ON product_listing_geographies(valid_until) WHERE valid_until IS NOT NULL;
CREATE INDEX product_listings_source_geography_cursor_idx ON product_listings(listing_source_id,product_listing_id);

-- Leased, coalesced dependency work. Claims never span external I/O; pages and
-- checkpoints commit together. A newer invalidation resets both cursors and fences claims.
CREATE TABLE listing_geography_work (
    work_id uuid PRIMARY KEY,
    dependency_kind text NOT NULL CHECK (dependency_kind IN ('SOURCE','LOCATION','RESOLUTION','LISTING','PROFILE')),
    dependency_id text NOT NULL CHECK (octet_length(dependency_id) BETWEEN 1 AND 255),
    generation bigint NOT NULL DEFAULT 1 CHECK (generation > 0),
    after_source uuid,
    after_listing uuid,
    lease_token uuid,
    lease_until timestamptz,
    pending boolean NOT NULL DEFAULT true,
    pending_since timestamptz NOT NULL DEFAULT now(),
    UNIQUE(dependency_kind,dependency_id),
    CHECK ((after_source IS NULL) = (after_listing IS NULL)),
    CHECK ((lease_token IS NULL) = (lease_until IS NULL))
);
CREATE INDEX listing_geography_work_pending_idx ON listing_geography_work(pending_since) WHERE pending;
CREATE TABLE listing_geography_wakeups (
    work_id uuid NOT NULL REFERENCES listing_geography_work(work_id),
    generation bigint NOT NULL CHECK (generation > 0),
    PRIMARY KEY(work_id,generation)
);
-- PostgreSQL 16 has no UUIDv7 generator. Use database time + random RFC4122 bits
-- only for technical wake-up identity, preserving the workspace's UUIDv7 contract.
CREATE FUNCTION listing_geography_uuid_v7() RETURNS uuid LANGUAGE sql VOLATILE AS $$
    SELECT (lpad(to_hex((extract(epoch FROM clock_timestamp())*1000)::bigint),12,'0') ||
        '7' || substr(replace(gen_random_uuid()::text,'-',''),14,3) ||
        '8' || substr(replace(gen_random_uuid()::text,'-',''),18,15))::uuid
$$;
CREATE FUNCTION enqueue_listing_geography(kind text, id text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE w uuid; g bigint;
BEGIN
    INSERT INTO listing_geography_work(work_id,dependency_kind,dependency_id)
    VALUES(listing_geography_uuid_v7(),kind,id)
    ON CONFLICT(dependency_kind,dependency_id) DO UPDATE SET
        generation=listing_geography_work.generation+1,after_source=NULL,after_listing=NULL,
        lease_token=NULL,lease_until=NULL,pending=true,
        pending_since=CASE WHEN listing_geography_work.pending THEN listing_geography_work.pending_since ELSE now() END
    RETURNING work_id,generation INTO w,g;
    INSERT INTO listing_geography_wakeups VALUES(w,g);
END $$;

CREATE FUNCTION signal_listing_geography() RETURNS trigger LANGUAGE plpgsql AS $$
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
    ELSIF TG_TABLE_NAME='geo_resolution_provider_controls' THEN
        IF NEW.storage_permitted IS DISTINCT FROM OLD.storage_permitted THEN
            PERFORM enqueue_listing_geography('PROFILE',NEW.profile);
        END IF;
    ELSIF TG_TABLE_NAME='product_listing_events' AND NEW.event_group='DOMAIN' THEN
        PERFORM enqueue_listing_geography('LISTING',NEW.product_listing_id::text);
    END IF;
    RETURN NEW;
END $$;

CREATE TRIGGER listing_geography_default_signal AFTER INSERT ON listing_source_location_default_changes FOR EACH ROW EXECUTE FUNCTION signal_listing_geography();
CREATE TRIGGER listing_geography_location_signal AFTER INSERT ON party_location_changes FOR EACH ROW EXECUTE FUNCTION signal_listing_geography();
CREATE TRIGGER listing_geography_resolution_signal AFTER INSERT ON geo_resolution_changes FOR EACH ROW EXECUTE FUNCTION signal_listing_geography();
CREATE TRIGGER listing_geography_provider_signal AFTER UPDATE ON geo_resolution_provider_controls FOR EACH ROW EXECUTE FUNCTION signal_listing_geography();
CREATE TRIGGER listing_geography_listing_signal AFTER INSERT ON product_listing_events FOR EACH ROW WHEN (NEW.event_group='DOMAIN') EXECUTE FUNCTION signal_listing_geography();

ALTER TABLE product_listing_events DROP CONSTRAINT product_listing_events_type_group_check;
ALTER TABLE product_listing_events ADD CONSTRAINT product_listing_events_type_group_check CHECK (
    (event_group='DOMAIN' AND event_type IN ('PRODUCT_LISTING_DISCOVERED','PRODUCT_LISTING_CHANGED')) OR
    (event_group='ENRICHMENT' AND event_type IN ('ENRICHMENT_EMBEDDED','ENRICHMENT_TRANSLATED_TITLES','ENRICHMENT_GEOGRAPHY_UPDATED'))
);
-- Initial rebuild uses source-level work, not one synchronous rewrite per listing.
SELECT enqueue_listing_geography('SOURCE',listing_source_id::text) FROM listing_sources;
