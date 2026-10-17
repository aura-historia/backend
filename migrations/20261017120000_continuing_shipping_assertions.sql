-- Shipping assertions continue until changed, withdrawn or their source loses authority.
-- Keep successful observations and all ownership, ordering and replay fences.
ALTER TABLE shipping_profiles DROP COLUMN fresh_until, DROP COLUMN expires_at;
ALTER TABLE product_listings
    DROP COLUMN delivery_fresh_until, DROP COLUMN delivery_expires_at;
ALTER TABLE product_listings ADD CONSTRAINT listing_delivery_evidence_window CHECK (
    (delivery_configuration IS NULL) = (delivery_verified_at IS NULL));
ALTER TABLE shipping_default_import_state DROP COLUMN fresh_until, DROP COLUMN expires_at;
ALTER TABLE listing_delivery_import_state DROP COLUMN fresh_until, DROP COLUMN expires_at;
ALTER TABLE product_listing_shipping DROP COLUMN valid_until;

CREATE OR REPLACE FUNCTION shipping_assertion_verification() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.terms IS DISTINCT FROM OLD.terms THEN
        NEW.verified_at := now();
    END IF;
    RETURN NEW;
END $$;

CREATE OR REPLACE FUNCTION listing_delivery_verification() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        IF NEW.delivery_configuration IS NOT NULL THEN NEW.delivery_verified_at := now(); END IF;
    ELSIF NEW.delivery_configuration IS DISTINCT FROM OLD.delivery_configuration THEN
        NEW.delivery_verified_at := CASE WHEN NEW.delivery_configuration IS NOT NULL THEN now() END;
    END IF;
    RETURN NEW;
END $$;

CREATE OR REPLACE FUNCTION shipping_default_manual_ownership() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP = 'INSERT' OR NEW.revision IS DISTINCT FROM OLD.revision THEN
  UPDATE shipping_default_import_state SET owner_hash=NULL,verified_at=NULL WHERE listing_source_id=NEW.listing_source_id;
 END IF;
 RETURN NEW;
END $$;

CREATE OR REPLACE FUNCTION listing_delivery_manual_ownership() RETURNS trigger LANGUAGE plpgsql AS $$
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

-- Rebuild derived shipping through ordinary CDC work under the continuing-assertion policy.
SELECT enqueue_listing_shipping('SOURCE', listing_source_id::text) FROM listing_sources;
