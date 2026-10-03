-- Disable WooCommerce ingestion that never had a usable webhook secret. Such
-- sources can be re-enabled atomically by the partner configuration PUT.
DELETE FROM listing_source_ingestion_methods method
WHERE method.ingestion_method = 'WOOCOMMERCE'
  AND NOT EXISTS (
      SELECT 1
      FROM listing_source_woocommerce_ingestion_configurations configuration
      WHERE configuration.listing_source_id = method.listing_source_id
        AND configuration.webhook_secret ~ '[^[:space:]]'
  );

DELETE FROM listing_source_woocommerce_ingestion_configurations configuration
WHERE NOT EXISTS (
    SELECT 1
    FROM listing_source_ingestion_methods method
    WHERE method.listing_source_id = configuration.listing_source_id
      AND method.ingestion_method = 'WOOCOMMERCE'
);

ALTER TABLE listing_source_woocommerce_ingestion_configurations
    ALTER COLUMN webhook_secret SET NOT NULL;

ALTER TABLE listing_source_woocommerce_ingestion_configurations
    ADD CONSTRAINT listing_source_woocommerce_webhook_secret_nonblank
        CHECK (webhook_secret ~ '[^[:space:]]');

CREATE FUNCTION ensure_listing_source_woocommerce_configuration_consistency()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    target_listing_source_id uuid;
    has_method boolean;
    has_configuration boolean;
BEGIN
    IF TG_OP = 'UPDATE' THEN
        IF OLD.listing_source_id <> NEW.listing_source_id THEN
            SELECT EXISTS (
                SELECT 1
                FROM listing_source_ingestion_methods
                WHERE listing_source_id = OLD.listing_source_id
                  AND ingestion_method = 'WOOCOMMERCE'
            ) INTO has_method;
            SELECT EXISTS (
                SELECT 1
                FROM listing_source_woocommerce_ingestion_configurations
                WHERE listing_source_id = OLD.listing_source_id
            ) INTO has_configuration;

            IF has_method <> has_configuration THEN
                RAISE EXCEPTION 'WooCommerce ingestion method and configuration must exist together'
                    USING ERRCODE = '23514', CONSTRAINT = 'listing_source_woocommerce_configuration_consistency';
            END IF;
        END IF;
    END IF;

    IF TG_OP = 'DELETE' THEN
        target_listing_source_id := OLD.listing_source_id;
    ELSE
        target_listing_source_id := NEW.listing_source_id;
    END IF;

    SELECT EXISTS (
        SELECT 1
        FROM listing_source_ingestion_methods
        WHERE listing_source_id = target_listing_source_id
          AND ingestion_method = 'WOOCOMMERCE'
    ) INTO has_method;
    SELECT EXISTS (
        SELECT 1
        FROM listing_source_woocommerce_ingestion_configurations
        WHERE listing_source_id = target_listing_source_id
    ) INTO has_configuration;

    IF has_method <> has_configuration THEN
        RAISE EXCEPTION 'WooCommerce ingestion method and configuration must exist together'
            USING ERRCODE = '23514', CONSTRAINT = 'listing_source_woocommerce_configuration_consistency';
    END IF;
    RETURN NULL;
END;
$$;

CREATE CONSTRAINT TRIGGER listing_source_woocommerce_method_configuration_consistency
AFTER INSERT OR UPDATE OR DELETE ON listing_source_ingestion_methods
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION ensure_listing_source_woocommerce_configuration_consistency();

CREATE CONSTRAINT TRIGGER listing_source_woocommerce_configuration_method_consistency
AFTER INSERT OR UPDATE OR DELETE ON listing_source_woocommerce_ingestion_configurations
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION ensure_listing_source_woocommerce_configuration_consistency();

ALTER TABLE access_tokens DROP CONSTRAINT access_tokens_scopes_check;
ALTER TABLE access_tokens ADD CONSTRAINT access_tokens_scopes_check CHECK (
    scopes <@ ARRAY[
        'product-listings:write',
        'listing-sources:write',
        'shops:read',
        'shops:write',
        'partner-shop-applications:write',
        'partner-shops:read',
        'partner-shops:write',
        'users:read',
        'users:write',
        'access-tokens:read',
        'access-tokens:write',
        'search-filters:write',
        'watchlist:read',
        'watchlist:write'
    ]::text[]
);

ALTER TABLE oauth_clients DROP CONSTRAINT oauth_clients_scopes_check;
ALTER TABLE oauth_clients ADD CONSTRAINT oauth_clients_scopes_check CHECK (
    scopes <@ ARRAY[
        'product-listings:write',
        'listing-sources:write',
        'shops:read',
        'shops:write',
        'partner-shop-applications:write',
        'partner-shops:read',
        'partner-shops:write',
        'users:read',
        'users:write',
        'access-tokens:read',
        'access-tokens:write',
        'search-filters:write',
        'watchlist:read',
        'watchlist:write'
    ]::text[]
);

ALTER TABLE oauth_authorization_codes DROP CONSTRAINT oauth_authorization_codes_scopes_check;
ALTER TABLE oauth_authorization_codes ADD CONSTRAINT oauth_authorization_codes_scopes_check CHECK (
    scopes <@ ARRAY[
        'product-listings:write',
        'listing-sources:write',
        'shops:read',
        'shops:write',
        'partner-shop-applications:write',
        'partner-shops:read',
        'partner-shops:write',
        'users:read',
        'users:write',
        'access-tokens:read',
        'access-tokens:write',
        'search-filters:write',
        'watchlist:read',
        'watchlist:write'
    ]::text[]
);

ALTER TABLE oauth_third_party_exchange_codes DROP CONSTRAINT oauth_third_party_exchange_codes_scopes_check;
ALTER TABLE oauth_third_party_exchange_codes ADD CONSTRAINT oauth_third_party_exchange_codes_scopes_check CHECK (
    scopes <@ ARRAY[
        'product-listings:write',
        'listing-sources:write',
        'shops:read',
        'shops:write',
        'partner-shop-applications:write',
        'partner-shops:read',
        'partner-shops:write',
        'users:read',
        'users:write',
        'access-tokens:read',
        'access-tokens:write',
        'search-filters:write',
        'watchlist:read',
        'watchlist:write'
    ]::text[]
);
