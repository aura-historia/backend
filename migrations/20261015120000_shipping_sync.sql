-- Protected evidence custody. These tables are excluded from ordinary CDC/public jobs.
CREATE TABLE shipping_sync_epochs (
    epoch uuid PRIMARY KEY,
    listing_source_id uuid NOT NULL,
    current_source_id uuid REFERENCES listing_sources(listing_source_id) ON DELETE SET NULL,
    integration_key text NOT NULL,
    integration jsonb NOT NULL,
    active boolean NOT NULL,
    live_sequence bigint NOT NULL DEFAULT 0 CHECK (live_sequence>=0),
    configuration_sequence bigint NOT NULL DEFAULT 0 CHECK (configuration_sequence>=0),
    created timestamptz NOT NULL DEFAULT now(),
    CHECK (current_source_id IS NULL OR current_source_id=listing_source_id)
);
CREATE TABLE shipping_sync_installations (
    listing_source_id uuid NOT NULL,
    integration_key text NOT NULL,
    epoch uuid NOT NULL REFERENCES shipping_sync_epochs(epoch),
    revision bigint NOT NULL CHECK (revision>0),
    PRIMARY KEY(listing_source_id,integration_key)
);
CREATE TABLE shipping_sync_scopes (
    epoch uuid NOT NULL REFERENCES shipping_sync_epochs(epoch),
    scope text NOT NULL CHECK (octet_length(scope) BETWEEN 1 AND 128),
    generation bigint NOT NULL DEFAULT 0 CHECK (generation>=0),
    live_sequence bigint NOT NULL DEFAULT 0 CHECK (live_sequence>=0),
    configuration_sequence bigint NOT NULL DEFAULT 0 CHECK (configuration_sequence>=0),
    dirty boolean NOT NULL DEFAULT false,
    PRIMARY KEY(epoch,scope)
);
CREATE TABLE shipping_sync_sessions (
    session_id uuid PRIMARY KEY,
    listing_source_id uuid NOT NULL,
    epoch uuid NOT NULL,
    scope text NOT NULL,
    fetch_key text NOT NULL CHECK (octet_length(fetch_key) BETWEEN 1 AND 128),
    mode text NOT NULL CHECK (mode IN ('SNAPSHOT','DELTA')),
    expected_generation bigint NOT NULL CHECK (expected_generation>=0),
    expected_sequence bigint NOT NULL CHECK (expected_sequence>=0),
    provider_version text NOT NULL CHECK (octet_length(provider_version) BETWEEN 1 AND 32),
    components boolean[] NOT NULL CHECK (array_length(components,1)=3 AND array_position(components,NULL) IS NULL AND true=ANY(components)),
    state text NOT NULL CHECK (state IN ('OPEN','READY','WAITING_DEPENDENCY','APPLIED','RECONCILE_REQUIRED','EXPIRED')),
    raw_complete boolean NOT NULL DEFAULT false,
    manifest_chunks integer CHECK (manifest_chunks BETWEEN 1 AND 16),
    manifest_records integer CHECK (manifest_records BETWEEN 0 AND 256),
    manifest_digest bytea CHECK (octet_length(manifest_digest)=32),
    generation bigint CHECK (generation>0),
    failure text,
    started_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    expires_at timestamptz NOT NULL DEFAULT (clock_timestamp()+interval '24 hours'),
    FOREIGN KEY(epoch,scope) REFERENCES shipping_sync_scopes(epoch,scope),
    UNIQUE(epoch,fetch_key),
    CHECK ((manifest_chunks IS NULL AND manifest_records IS NULL AND manifest_digest IS NULL) OR
        (manifest_chunks IS NOT NULL AND manifest_records IS NOT NULL AND manifest_digest IS NOT NULL)),
    CHECK ((state='APPLIED')=(generation IS NOT NULL)),
    CHECK (state NOT IN ('READY','WAITING_DEPENDENCY','APPLIED','RECONCILE_REQUIRED') OR raw_complete),
    CHECK (NOT raw_complete OR manifest_chunks IS NOT NULL)
);
CREATE INDEX shipping_sync_pending ON shipping_sync_sessions(epoch,session_id)
    WHERE state IN ('READY','WAITING_DEPENDENCY');
-- Address-free application signals are retained independently of captured content.
CREATE TABLE shipping_sync_changes (
    session_id uuid PRIMARY KEY REFERENCES shipping_sync_sessions(session_id),
    schema_version smallint NOT NULL DEFAULT 1 CHECK (schema_version=1),
    listing_source_id uuid NOT NULL,
    epoch uuid NOT NULL REFERENCES shipping_sync_epochs(epoch),
    generation bigint NOT NULL CHECK (generation>0),
    recorded timestamptz NOT NULL DEFAULT now()
);
CREATE TRIGGER shipping_sync_changes_immutable BEFORE UPDATE OR DELETE ON shipping_sync_changes
    FOR EACH ROW EXECUTE FUNCTION reject_shipping_history_mutation();
CREATE TABLE shipping_sync_chunks (
    session_id uuid NOT NULL REFERENCES shipping_sync_sessions(session_id),
    chunk_index integer NOT NULL CHECK (chunk_index BETWEEN 0 AND 15),
    digest bytea NOT NULL CHECK (octet_length(digest)=32),
    records integer NOT NULL CHECK (records BETWEEN 0 AND 16),
    payload jsonb,
    erased_at timestamptz,
    PRIMARY KEY(session_id,chunk_index),
    CHECK ((payload IS NOT NULL AND jsonb_typeof(payload)='object' AND erased_at IS NULL AND octet_length(payload::text)<=131072)
        OR (payload IS NULL AND erased_at IS NOT NULL))
);
CREATE FUNCTION protect_shipping_sync_chunk() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='UPDATE' AND OLD.payload IS NOT NULL AND NEW.payload IS NULL AND NEW.erased_at IS NOT NULL
        AND (to_jsonb(OLD)-'payload'-'erased_at')=(to_jsonb(NEW)-'payload'-'erased_at')
        AND EXISTS(SELECT 1 FROM shipping_sync_sessions s WHERE s.session_id=OLD.session_id
            AND s.started_at<clock_timestamp()-interval '30 days') THEN
        RETURN NEW;
    END IF;
    RAISE EXCEPTION 'shipping sync chunks are immutable except retained content erasure';
END $$;
CREATE TRIGGER shipping_sync_chunks_immutable BEFORE UPDATE OR DELETE ON shipping_sync_chunks
    FOR EACH ROW EXECUTE FUNCTION protect_shipping_sync_chunk();
CREATE TABLE shipping_sync_claims (
    epoch uuid NOT NULL REFERENCES shipping_sync_epochs(epoch),
    object_kind text NOT NULL CHECK (object_kind IN ('PROFILE','DEFAULT','ITEM')),
    external_key text NOT NULL CHECK (octet_length(external_key) BETWEEN 1 AND 512),
    scope text NOT NULL,
    observed_at timestamptz NOT NULL,
    fresh_until timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    applied jsonb NOT NULL CHECK (jsonb_typeof(applied)='object'),
    tombstoned boolean NOT NULL,
    PRIMARY KEY(epoch,object_kind,external_key),
    FOREIGN KEY(epoch,scope) REFERENCES shipping_sync_scopes(epoch,scope),
    CHECK (observed_at<=fresh_until AND fresh_until<=expires_at)
);
CREATE INDEX shipping_sync_claims_scope ON shipping_sync_claims(epoch,scope,object_kind,external_key);

ALTER TABLE shipping_profiles ADD COLUMN sync_epoch uuid REFERENCES shipping_sync_epochs(epoch);
ALTER TABLE shipping_source_defaults ADD COLUMN sync_epoch uuid REFERENCES shipping_sync_epochs(epoch),
    ADD COLUMN sync_observed_at timestamptz, ADD COLUMN sync_fresh_until timestamptz, ADD COLUMN sync_expires_at timestamptz,
    ADD CONSTRAINT shipping_default_sync_window CHECK (
        (sync_observed_at IS NULL AND sync_fresh_until IS NULL AND sync_expires_at IS NULL) OR
        (sync_epoch IS NOT NULL AND sync_observed_at IS NOT NULL AND sync_fresh_until IS NOT NULL AND sync_expires_at IS NOT NULL AND sync_observed_at<=sync_fresh_until AND sync_fresh_until<=sync_expires_at));
ALTER TABLE product_listings
    ADD COLUMN delivery_selection_epoch uuid REFERENCES shipping_sync_epochs(epoch),
    ADD COLUMN delivery_inputs_epoch uuid REFERENCES shipping_sync_epochs(epoch),
    ADD COLUMN delivery_shipping_epoch uuid REFERENCES shipping_sync_epochs(epoch),
    ADD COLUMN delivery_collection_epoch uuid REFERENCES shipping_sync_epochs(epoch),
    ADD COLUMN delivery_selection_observed_at timestamptz,
    ADD COLUMN delivery_selection_fresh_until timestamptz,
    ADD COLUMN delivery_selection_expires_at timestamptz,
    ADD COLUMN delivery_inputs_observed_at timestamptz,
    ADD COLUMN delivery_inputs_fresh_until timestamptz,
    ADD COLUMN delivery_inputs_expires_at timestamptz,
    ADD COLUMN delivery_shipping_observed_at timestamptz,
    ADD COLUMN delivery_shipping_fresh_until timestamptz,
    ADD COLUMN delivery_shipping_expires_at timestamptz,
    ADD COLUMN delivery_collection_observed_at timestamptz,
    ADD COLUMN delivery_collection_fresh_until timestamptz,
    ADD COLUMN delivery_collection_expires_at timestamptz;

ALTER TABLE product_listings ADD CONSTRAINT delivery_selection_window CHECK (
    (delivery_selection_observed_at IS NULL AND delivery_selection_fresh_until IS NULL AND delivery_selection_expires_at IS NULL) OR
    (delivery_selection_epoch IS NOT NULL AND delivery_selection_observed_at IS NOT NULL AND delivery_selection_fresh_until IS NOT NULL AND delivery_selection_expires_at IS NOT NULL AND delivery_selection_observed_at<=delivery_selection_fresh_until AND delivery_selection_fresh_until<=delivery_selection_expires_at));
ALTER TABLE product_listings ADD CONSTRAINT delivery_inputs_window CHECK (
    (delivery_inputs_observed_at IS NULL AND delivery_inputs_fresh_until IS NULL AND delivery_inputs_expires_at IS NULL) OR
    (delivery_inputs_epoch IS NOT NULL AND delivery_inputs_observed_at IS NOT NULL AND delivery_inputs_fresh_until IS NOT NULL AND delivery_inputs_expires_at IS NOT NULL AND delivery_inputs_observed_at<=delivery_inputs_fresh_until AND delivery_inputs_fresh_until<=delivery_inputs_expires_at));
ALTER TABLE product_listings ADD CONSTRAINT delivery_shipping_window CHECK (
    (delivery_shipping_observed_at IS NULL AND delivery_shipping_fresh_until IS NULL AND delivery_shipping_expires_at IS NULL) OR
    (delivery_shipping_epoch IS NOT NULL AND delivery_shipping_observed_at IS NOT NULL AND delivery_shipping_fresh_until IS NOT NULL AND delivery_shipping_expires_at IS NOT NULL AND delivery_shipping_observed_at<=delivery_shipping_fresh_until AND delivery_shipping_fresh_until<=delivery_shipping_expires_at));
ALTER TABLE product_listings ADD CONSTRAINT delivery_collection_window CHECK (
    (delivery_collection_observed_at IS NULL AND delivery_collection_fresh_until IS NULL AND delivery_collection_expires_at IS NULL) OR
    (delivery_collection_epoch IS NOT NULL AND delivery_collection_observed_at IS NOT NULL AND delivery_collection_fresh_until IS NOT NULL AND delivery_collection_expires_at IS NOT NULL AND delivery_collection_observed_at<=delivery_collection_fresh_until AND delivery_collection_fresh_until<=delivery_collection_expires_at));

-- Explicit management edits release acquisition ownership for exactly the components edited.
-- Sync application writes its ownership only after the normal aggregate write in the same UoW.
CREATE FUNCTION release_shipping_sync_ownership() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_TABLE_NAME='shipping_profiles' THEN
        IF NEW.terms IS DISTINCT FROM OLD.terms OR NEW.lifecycle IS DISTINCT FROM OLD.lifecycle THEN NEW.sync_epoch:=NULL; END IF;
    ELSIF TG_TABLE_NAME='shipping_source_defaults' THEN
        IF NEW.revision IS DISTINCT FROM OLD.revision THEN NEW.sync_epoch:=NULL; NEW.sync_observed_at:=NULL; NEW.sync_fresh_until:=NULL; NEW.sync_expires_at:=NULL; END IF;
    ELSE
        IF NEW.delivery_configuration IS DISTINCT FROM OLD.delivery_configuration THEN
            IF COALESCE(NEW.delivery_configuration->'selection','{"kind":"INHERIT_SOURCE_DEFAULT"}'::jsonb) IS DISTINCT FROM COALESCE(OLD.delivery_configuration->'selection','{"kind":"INHERIT_SOURCE_DEFAULT"}'::jsonb) THEN NEW.delivery_selection_epoch:=NULL; NEW.delivery_selection_observed_at:=NULL; NEW.delivery_selection_fresh_until:=NULL; NEW.delivery_selection_expires_at:=NULL; END IF;
            IF COALESCE(NEW.delivery_configuration->'rate_inputs','{"classes":null,"variants":null}'::jsonb) IS DISTINCT FROM COALESCE(OLD.delivery_configuration->'rate_inputs','{"classes":null,"variants":null}'::jsonb) THEN NEW.delivery_inputs_epoch:=NULL; NEW.delivery_inputs_observed_at:=NULL; NEW.delivery_inputs_fresh_until:=NULL; NEW.delivery_inputs_expires_at:=NULL; END IF;
            IF COALESCE(NEW.delivery_configuration->'shipping','{"kind":"INHERIT"}'::jsonb) IS DISTINCT FROM COALESCE(OLD.delivery_configuration->'shipping','{"kind":"INHERIT"}'::jsonb) THEN NEW.delivery_shipping_epoch:=NULL; NEW.delivery_shipping_observed_at:=NULL; NEW.delivery_shipping_fresh_until:=NULL; NEW.delivery_shipping_expires_at:=NULL; END IF;
            IF COALESCE(NEW.delivery_configuration->'collection','{"kind":"INHERIT"}'::jsonb) IS DISTINCT FROM COALESCE(OLD.delivery_configuration->'collection','{"kind":"INHERIT"}'::jsonb) THEN NEW.delivery_collection_epoch:=NULL; NEW.delivery_collection_observed_at:=NULL; NEW.delivery_collection_fresh_until:=NULL; NEW.delivery_collection_expires_at:=NULL; END IF;
        END IF;
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER shipping_sync_profile_ownership BEFORE UPDATE ON shipping_profiles FOR EACH ROW EXECUTE FUNCTION release_shipping_sync_ownership();
CREATE TRIGGER shipping_sync_default_ownership BEFORE UPDATE ON shipping_source_defaults FOR EACH ROW EXECUTE FUNCTION release_shipping_sync_ownership();
CREATE TRIGGER shipping_sync_item_ownership BEFORE UPDATE ON product_listings FOR EACH ROW EXECUTE FUNCTION release_shipping_sync_ownership();

CREATE FUNCTION revoke_shipping_sync_configuration() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE method_prefix text;
BEGIN
    IF TG_TABLE_NAME='listing_source_shopify_ingestion_configurations' THEN method_prefix:='SHOPIFY:';
    ELSIF TG_TABLE_NAME='listing_source_woocommerce_ingestion_configurations' THEN method_prefix:='WOOCOMMERCE:';
    ELSE method_prefix:=OLD.ingestion_method || ':'; END IF;
    UPDATE shipping_sync_epochs SET active=false WHERE listing_source_id=OLD.listing_source_id
        AND integration_key LIKE method_prefix || '%';
    RETURN NULL;
END $$;
-- Any provider configuration replacement, removal or method revocation fences old acquisition.
CREATE TRIGGER shipping_sync_shopify_configuration AFTER UPDATE OR DELETE ON listing_source_shopify_ingestion_configurations FOR EACH ROW EXECUTE FUNCTION revoke_shipping_sync_configuration();
CREATE TRIGGER shipping_sync_woocommerce_configuration AFTER UPDATE OR DELETE ON listing_source_woocommerce_ingestion_configurations FOR EACH ROW EXECUTE FUNCTION revoke_shipping_sync_configuration();
CREATE TRIGGER shipping_sync_ingestion_method AFTER DELETE ON listing_source_ingestion_methods FOR EACH ROW EXECUTE FUNCTION revoke_shipping_sync_configuration();
