-- Registered OAuth clients and the credentials issued to them or created by users may carry
-- the admin read scope for content notices. Metadata-document clients and their connections
-- stay limited to the ordinary-user scope vocabulary (docs/oauth.md), so their constraints
-- are unchanged. See docs/content-notices.md.

ALTER TABLE access_tokens
    DROP CONSTRAINT access_tokens_scopes_check,
    ADD CONSTRAINT access_tokens_scopes_check CHECK (
        scopes <@ ARRAY[
            'auctions:read',
            'auctions:write',
            'listing-sources:read',
            'parties:read',
            'parties:write',
            'partnership-applications:read',
            'partnership-applications:write',
            'partnerships:read',
            'partnerships:write',
            'admin-overview:read',
            'content-notices:read',
            'search-filters:read',
            'notifications:read',
            'notifications:write',
            'product-listings:read',
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

ALTER TABLE oauth_clients
    DROP CONSTRAINT oauth_clients_scopes_check,
    ADD CONSTRAINT oauth_clients_scopes_check CHECK (
        scopes <@ ARRAY[
            'auctions:read',
            'auctions:write',
            'listing-sources:read',
            'parties:read',
            'parties:write',
            'partnership-applications:read',
            'partnership-applications:write',
            'partnerships:read',
            'partnerships:write',
            'admin-overview:read',
            'content-notices:read',
            'search-filters:read',
            'notifications:read',
            'notifications:write',
            'product-listings:read',
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

ALTER TABLE oauth_authorization_codes
    DROP CONSTRAINT oauth_authorization_codes_scopes_check,
    ADD CONSTRAINT oauth_authorization_codes_scopes_check CHECK (
        scopes <@ ARRAY[
            'auctions:read',
            'auctions:write',
            'listing-sources:read',
            'parties:read',
            'parties:write',
            'partnership-applications:read',
            'partnership-applications:write',
            'partnerships:read',
            'partnerships:write',
            'admin-overview:read',
            'content-notices:read',
            'search-filters:read',
            'notifications:read',
            'notifications:write',
            'product-listings:read',
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

ALTER TABLE oauth_third_party_exchange_codes
    DROP CONSTRAINT oauth_third_party_exchange_codes_scopes_check,
    ADD CONSTRAINT oauth_third_party_exchange_codes_scopes_check CHECK (
        scopes <@ ARRAY[
            'auctions:read',
            'auctions:write',
            'listing-sources:read',
            'parties:read',
            'parties:write',
            'partnership-applications:read',
            'partnership-applications:write',
            'partnerships:read',
            'partnerships:write',
            'admin-overview:read',
            'content-notices:read',
            'search-filters:read',
            'notifications:read',
            'notifications:write',
            'product-listings:read',
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
