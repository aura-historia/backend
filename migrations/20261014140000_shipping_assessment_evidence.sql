-- Assertion age is independent of unrelated edits and failed acquisition attempts.
ALTER TABLE shipping_profiles
    ADD COLUMN verified_at timestamptz,
    ADD COLUMN fresh_until timestamptz,
    ADD COLUMN expires_at timestamptz;
UPDATE shipping_profiles SET verified_at=created;
ALTER TABLE shipping_profiles ALTER COLUMN verified_at SET DEFAULT now(),
    ALTER COLUMN verified_at SET NOT NULL,
    ADD CONSTRAINT shipping_profile_evidence_window CHECK (
        (fresh_until IS NULL OR fresh_until >= verified_at) AND
        (expires_at IS NULL OR (expires_at >= verified_at AND
            (fresh_until IS NULL OR expires_at >= fresh_until))));
ALTER TABLE product_listings ADD COLUMN delivery_verified_at timestamptz,
    ADD COLUMN delivery_fresh_until timestamptz,
    ADD COLUMN delivery_expires_at timestamptz;
UPDATE product_listings SET delivery_verified_at=created WHERE delivery_configuration IS NOT NULL;
ALTER TABLE product_listings ADD CONSTRAINT listing_delivery_evidence_window CHECK (
    ((delivery_configuration IS NULL AND delivery_verified_at IS NULL AND
        delivery_fresh_until IS NULL AND delivery_expires_at IS NULL) OR
     (delivery_configuration IS NOT NULL AND delivery_verified_at IS NOT NULL)) AND
    (delivery_fresh_until IS NULL OR delivery_fresh_until >= delivery_verified_at) AND
    (delivery_expires_at IS NULL OR (delivery_expires_at >= delivery_verified_at AND
        (delivery_fresh_until IS NULL OR delivery_expires_at >= delivery_fresh_until))));

-- Source-authored replacements verify the new assertion. Equal writes, evidence correction,
-- retirement/restoration, raw recrawls and unrelated canonical edits do not renew it.
CREATE FUNCTION shipping_assertion_verification() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.terms IS DISTINCT FROM OLD.terms THEN
        NEW.verified_at := now();
        NEW.fresh_until := NULL;
        NEW.expires_at := NULL;
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER shipping_assertion_verification BEFORE UPDATE ON shipping_profiles
    FOR EACH ROW EXECUTE FUNCTION shipping_assertion_verification();
CREATE FUNCTION listing_delivery_verification() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        IF NEW.delivery_configuration IS NOT NULL THEN NEW.delivery_verified_at := now(); END IF;
    ELSIF NEW.delivery_configuration IS DISTINCT FROM OLD.delivery_configuration THEN
        NEW.delivery_verified_at := CASE WHEN NEW.delivery_configuration IS NOT NULL THEN now() END;
        NEW.delivery_fresh_until := NULL;
        NEW.delivery_expires_at := NULL;
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER listing_delivery_verification BEFORE INSERT OR UPDATE ON product_listings
    FOR EACH ROW EXECUTE FUNCTION listing_delivery_verification();
