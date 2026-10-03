ALTER TABLE listing_source_woocommerce_ingestion_configurations
    ALTER COLUMN webhook_secret SET NOT NULL;

ALTER TABLE listing_source_woocommerce_ingestion_configurations
    ADD CONSTRAINT listing_source_woocommerce_webhook_secret_nonblank
        CHECK (webhook_secret ~ '[^[:space:]]');


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
