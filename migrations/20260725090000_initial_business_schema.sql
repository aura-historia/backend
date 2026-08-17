CREATE TABLE users (
    user_id uuid PRIMARY KEY,
    email text NOT NULL UNIQUE,
    first_name text,
    last_name text,
    language text,
    currency text,
    measurement_unit text,
    prohibited_content_consent boolean NOT NULL DEFAULT false,
    tier text NOT NULL,
    role text NOT NULL,
    stripe_customer_id text UNIQUE,
    structured_address_addressline text,
    structured_address_addressline_extra text,
    structured_address_locality text,
    structured_address_region text,
    structured_address_postal_code text,
    structured_address_country text,
    geo_address_lat double precision,
    geo_address_lon double precision,
    version bigint NOT NULL DEFAULT 1,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT users_language_check CHECK (language IS NULL OR language IN ('de', 'en', 'fr', 'es', 'it', 'zh', 'pt', 'pl', 'tr', 'nl', 'cs', 'ja', 'ru', 'ar')),
    CONSTRAINT users_currency_check CHECK (currency IS NULL OR currency IN ('EUR', 'GBP', 'USD', 'AUD', 'CAD', 'NZD', 'CNY', 'BRL', 'PLN', 'TRY', 'JPY', 'CZK', 'RUB', 'AED', 'SAR', 'HKD', 'SGD', 'CHF')),
    CONSTRAINT users_measurement_unit_check CHECK (measurement_unit IS NULL OR measurement_unit IN ('METRIC', 'IMPERIAL')),
    CONSTRAINT users_tier_check CHECK (tier IN ('FREE', 'PRO', 'ULTIMATE')),
    CONSTRAINT users_role_check CHECK (role IN ('USER', 'ADMIN')),
    CONSTRAINT users_geo_pair_check CHECK ((geo_address_lat IS NULL) = (geo_address_lon IS NULL)),
    CONSTRAINT users_geo_lat_range CHECK (geo_address_lat IS NULL OR geo_address_lat BETWEEN -90 AND 90),
    CONSTRAINT users_geo_lon_range CHECK (geo_address_lon IS NULL OR geo_address_lon BETWEEN -180 AND 180),
    CONSTRAINT users_version_positive CHECK (version >= 1)
);

CREATE INDEX users_created_idx ON users (created DESC);

CREATE TABLE shops (
    shop_id uuid PRIMARY KEY,
    shop_slug_id text NOT NULL UNIQUE,
    name text NOT NULL,
    shop_type text NOT NULL,
    partner_status text NOT NULL,
    lifecycle text NOT NULL DEFAULT 'DRAFTED',
    shop_domains text[] NOT NULL DEFAULT '{}',
    shopify_domain text UNIQUE,
    shopify_currency text,
    shopify_language text,
    woocommerce_webhook_secret text,
    woocommerce_currency text,
    woocommerce_language text,
    url text,
    view_url text,
    image text,
    structured_address_addressline text,
    structured_address_addressline_extra text,
    structured_address_locality text,
    structured_address_region text,
    structured_address_postal_code text,
    structured_address_country text,
    geo_address_lat double precision,
    geo_address_lon double precision,
    phone text,
    email text,
    affiliate_configuration jsonb,
    version bigint NOT NULL DEFAULT 1,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT shops_type_check CHECK (shop_type IN ('AUCTION_HOUSE', 'AUCTION_PLATFORM', 'COMMERCIAL_DEALER', 'MARKETPLACE')),
    CONSTRAINT shops_partner_status_check CHECK (partner_status IN ('SCRAPED', 'PARTNERED')),
    CONSTRAINT shops_lifecycle_check CHECK (lifecycle IN ('DRAFTED', 'PUBLISHED', 'DISCARDED')),
    CONSTRAINT shops_shopify_currency_check CHECK (shopify_currency IS NULL OR shopify_currency IN ('EUR', 'GBP', 'USD', 'AUD', 'CAD', 'NZD', 'CNY', 'BRL', 'PLN', 'TRY', 'JPY', 'CZK', 'RUB', 'AED', 'SAR', 'HKD', 'SGD', 'CHF')),
    CONSTRAINT shops_woocommerce_currency_check CHECK (woocommerce_currency IS NULL OR woocommerce_currency IN ('EUR', 'GBP', 'USD', 'AUD', 'CAD', 'NZD', 'CNY', 'BRL', 'PLN', 'TRY', 'JPY', 'CZK', 'RUB', 'AED', 'SAR', 'HKD', 'SGD', 'CHF')),
    CONSTRAINT shops_shopify_language_check CHECK (shopify_language IS NULL OR shopify_language IN ('de', 'en', 'fr', 'es', 'it', 'zh', 'pt', 'pl', 'tr', 'nl', 'cs', 'ja', 'ru', 'ar')),
    CONSTRAINT shops_woocommerce_language_check CHECK (woocommerce_language IS NULL OR woocommerce_language IN ('de', 'en', 'fr', 'es', 'it', 'zh', 'pt', 'pl', 'tr', 'nl', 'cs', 'ja', 'ru', 'ar')),
    CONSTRAINT shops_geo_pair_check CHECK ((geo_address_lat IS NULL) = (geo_address_lon IS NULL)),
    CONSTRAINT shops_geo_lat_range CHECK (geo_address_lat IS NULL OR geo_address_lat BETWEEN -90 AND 90),
    CONSTRAINT shops_geo_lon_range CHECK (geo_address_lon IS NULL OR geo_address_lon BETWEEN -180 AND 180),
    CONSTRAINT shops_affiliate_configuration_object CHECK (affiliate_configuration IS NULL OR jsonb_typeof(affiliate_configuration) = 'object'),
    CONSTRAINT shops_version_positive CHECK (version >= 1)
);

CREATE INDEX shops_shop_domains_gin_idx ON shops USING gin (shop_domains);
CREATE INDEX shops_partner_status_updated_idx ON shops (partner_status, updated DESC);

CREATE TABLE user_partner_shops (
    user_id uuid NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    shop_id uuid NOT NULL REFERENCES shops(shop_id) ON DELETE CASCADE,
    created timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, shop_id)
);

CREATE INDEX user_partner_shops_shop_id_idx ON user_partner_shops (shop_id);

CREATE TABLE partner_shop_applications (
    partner_shop_application_id uuid PRIMARY KEY,
    applicant_user_id uuid NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    business_state text NOT NULL,
    payload_type text NOT NULL,
    shop_id uuid NOT NULL REFERENCES shops(shop_id) ON DELETE RESTRICT,
    version bigint NOT NULL DEFAULT 1,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT partner_shop_applications_business_state_check CHECK (business_state IN ('SUBMITTED', 'IN_REVIEW', 'REJECTED', 'APPROVED', 'WITHDRAWN')),
    CONSTRAINT partner_shop_applications_payload_type_check CHECK (payload_type IN ('EXISTING', 'NEW')),
    CONSTRAINT partner_shop_applications_version_positive CHECK (version >= 1)
);

CREATE INDEX partner_shop_applications_applicant_created_idx ON partner_shop_applications (applicant_user_id, created DESC);
CREATE INDEX partner_shop_applications_business_state_created_idx ON partner_shop_applications (business_state, created DESC);
CREATE INDEX partner_shop_applications_shop_id_idx ON partner_shop_applications (shop_id);

CREATE TABLE fx_rates (
    fx_rate_id uuid PRIMARY KEY,
    generation bigint GENERATED ALWAYS AS IDENTITY UNIQUE,
    captured_at timestamptz NOT NULL,
    source text NOT NULL,
    source_event_id text NOT NULL UNIQUE,
    created timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX fx_rates_captured_at_generation_idx
    ON fx_rates (captured_at DESC, generation DESC);

CREATE TABLE fx_rate_quotes (
    fx_rate_id uuid NOT NULL REFERENCES fx_rates(fx_rate_id) ON DELETE RESTRICT,
    currency text NOT NULL,
    units_per_eur bigint NOT NULL CHECK (units_per_eur > 0),
    PRIMARY KEY (fx_rate_id, currency),
    CONSTRAINT fx_rate_quotes_currency_check CHECK (currency IN ('EUR', 'GBP', 'USD', 'AUD', 'CAD', 'NZD', 'CNY', 'BRL', 'PLN', 'TRY', 'JPY', 'CZK', 'RUB', 'AED', 'SAR', 'HKD', 'SGD', 'CHF'))
);

CREATE TABLE products (
    product_id uuid PRIMARY KEY,
    product_slug_id text NOT NULL,
    event_id uuid NOT NULL,
    shop_id uuid NOT NULL REFERENCES shops(shop_id),
    seller_id uuid NOT NULL REFERENCES shops(shop_id),
    shops_product_id text NOT NULL,
    structured_address_addressline text,
    structured_address_addressline_extra text,
    structured_address_locality text,
    structured_address_region text,
    structured_address_postal_code text,
    structured_address_country text,
    geo_address_lat double precision,
    geo_address_lon double precision,
    title_text text,
    title_language text,
    description_text text,
    description_language text,
    price_amount bigint,
    price_currency text,
    price_estimate_min_amount bigint,
    price_estimate_min_currency text,
    price_estimate_max_amount bigint,
    price_estimate_max_currency text,
    sale_fx_rate_id uuid REFERENCES fx_rates(fx_rate_id) ON DELETE RESTRICT,
    sold_at timestamptz,
    state text NOT NULL,
    lifecycle text NOT NULL,
    url text NOT NULL,
    product_images jsonb NOT NULL DEFAULT '[]',
    embedding real[],
    projection_version bigint NOT NULL DEFAULT 1,
    auction_start timestamptz,
    auction_end timestamptz,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT products_shop_product_unique UNIQUE (shop_id, shops_product_id),
    CONSTRAINT products_slug_unique UNIQUE (product_slug_id),
    CONSTRAINT products_state_check CHECK (state IN ('LISTED', 'AVAILABLE', 'RESERVED', 'SOLD', 'REMOVED', 'UNKNOWN')),
    CONSTRAINT products_lifecycle_check CHECK (lifecycle IN ('ACTIVE', 'DELETED')),
    CONSTRAINT products_title_pair_check CHECK ((title_text IS NULL) = (title_language IS NULL)),
    CONSTRAINT products_description_pair_check CHECK ((description_text IS NULL) = (description_language IS NULL)),
    CONSTRAINT products_title_language_check CHECK (title_language IS NULL OR title_language IN ('de', 'en', 'fr', 'es', 'it', 'zh', 'pt', 'pl', 'tr', 'nl', 'cs', 'ja', 'ru', 'ar')),
    CONSTRAINT products_description_language_check CHECK (description_language IS NULL OR description_language IN ('de', 'en', 'fr', 'es', 'it', 'zh', 'pt', 'pl', 'tr', 'nl', 'cs', 'ja', 'ru', 'ar')),
    CONSTRAINT products_price_pair_check CHECK ((price_amount IS NULL) = (price_currency IS NULL)),
    CONSTRAINT products_price_estimate_min_pair_check CHECK ((price_estimate_min_amount IS NULL) = (price_estimate_min_currency IS NULL)),
    CONSTRAINT products_price_estimate_max_pair_check CHECK ((price_estimate_max_amount IS NULL) = (price_estimate_max_currency IS NULL)),
    CONSTRAINT products_price_amount_nonnegative CHECK (price_amount IS NULL OR price_amount >= 0),
    CONSTRAINT products_price_estimate_min_amount_nonnegative CHECK (price_estimate_min_amount IS NULL OR price_estimate_min_amount >= 0),
    CONSTRAINT products_price_estimate_max_amount_nonnegative CHECK (price_estimate_max_amount IS NULL OR price_estimate_max_amount >= 0),
    CONSTRAINT products_price_currency_check CHECK (price_currency IS NULL OR price_currency IN ('EUR', 'GBP', 'USD', 'AUD', 'CAD', 'NZD', 'CNY', 'BRL', 'PLN', 'TRY', 'JPY', 'CZK', 'RUB', 'AED', 'SAR', 'HKD', 'SGD', 'CHF')),
    CONSTRAINT products_price_estimate_min_currency_check CHECK (price_estimate_min_currency IS NULL OR price_estimate_min_currency IN ('EUR', 'GBP', 'USD', 'AUD', 'CAD', 'NZD', 'CNY', 'BRL', 'PLN', 'TRY', 'JPY', 'CZK', 'RUB', 'AED', 'SAR', 'HKD', 'SGD', 'CHF')),
    CONSTRAINT products_price_estimate_max_currency_check CHECK (price_estimate_max_currency IS NULL OR price_estimate_max_currency IN ('EUR', 'GBP', 'USD', 'AUD', 'CAD', 'NZD', 'CNY', 'BRL', 'PLN', 'TRY', 'JPY', 'CZK', 'RUB', 'AED', 'SAR', 'HKD', 'SGD', 'CHF')),
    CONSTRAINT products_sale_valuation_pair_check CHECK ((sale_fx_rate_id IS NULL) = (sold_at IS NULL)),
    CONSTRAINT products_sold_sale_valuation_check CHECK (state <> 'SOLD' OR sale_fx_rate_id IS NOT NULL),
    CONSTRAINT products_geo_pair_check CHECK ((geo_address_lat IS NULL) = (geo_address_lon IS NULL)),
    CONSTRAINT products_geo_lat_range CHECK (geo_address_lat IS NULL OR geo_address_lat BETWEEN -90 AND 90),
    CONSTRAINT products_geo_lon_range CHECK (geo_address_lon IS NULL OR geo_address_lon BETWEEN -180 AND 180),
    CONSTRAINT products_images_array CHECK (jsonb_typeof(product_images) = 'array'),
    CONSTRAINT products_embedding_dimension_check CHECK (embedding IS NULL OR (array_ndims(embedding) = 1 AND cardinality(embedding) = 768)),
    CONSTRAINT products_projection_version_positive CHECK (projection_version >= 1),
    CONSTRAINT products_auction_order_check CHECK (auction_start IS NULL OR auction_end IS NULL OR auction_start <= auction_end)
);

CREATE INDEX products_seller_id_idx ON products (seller_id);
CREATE INDEX products_lifecycle_updated_idx ON products (lifecycle, updated DESC);
CREATE INDEX products_sale_fx_rate_id_idx ON products (sale_fx_rate_id);

CREATE TABLE product_translations (
    product_id uuid NOT NULL REFERENCES products(product_id) ON DELETE CASCADE,
    source_event_id uuid NOT NULL,
    language text NOT NULL,
    title text,
    description text,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (product_id, language),
    CONSTRAINT product_translations_language_check CHECK (language IN ('de', 'en', 'fr', 'es', 'it', 'zh', 'pt', 'pl', 'tr', 'nl', 'cs', 'ja', 'ru', 'ar')),
    CONSTRAINT product_translations_has_content CHECK (title IS NOT NULL OR description IS NOT NULL)
);

CREATE INDEX product_translations_language_idx ON product_translations (language);

CREATE TABLE product_events (
    event_id uuid PRIMARY KEY,
    product_id uuid NOT NULL REFERENCES products(product_id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
    event_type text NOT NULL,
    event_group text NOT NULL,
    event_type_schema_version int NOT NULL DEFAULT 1,
    payload jsonb NOT NULL,
    event_time timestamptz NOT NULL,
    created timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT product_events_product_event_unique UNIQUE (product_id, event_id),
    CONSTRAINT product_events_group_check CHECK (event_group IN ('DOMAIN', 'ENRICHMENT', 'POLICY', 'LIFECYCLE')),
    CONSTRAINT product_events_schema_version_positive CHECK (event_type_schema_version >= 1),
    CONSTRAINT product_events_payload_object CHECK (jsonb_typeof(payload) = 'object')
);

ALTER TABLE products
    ADD CONSTRAINT products_current_event_same_product_fkey
    FOREIGN KEY (product_id, event_id)
    REFERENCES product_events(product_id, event_id)
    DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE product_translations
    ADD CONSTRAINT product_translations_source_event_fkey
    FOREIGN KEY (product_id, source_event_id)
    REFERENCES product_events(product_id, event_id)
    DEFERRABLE INITIALLY DEFERRED;

CREATE INDEX product_events_product_time_idx ON product_events (product_id, event_time ASC);
CREATE INDEX product_translations_source_event_idx
    ON product_translations (product_id, source_event_id);
CREATE INDEX product_events_type_time_idx ON product_events (event_type, event_time ASC);

CREATE TABLE product_watchlist (
    user_id uuid NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    product_id uuid NOT NULL REFERENCES products(product_id) ON DELETE CASCADE,
    notifications boolean NOT NULL DEFAULT true,
    state text NOT NULL,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, product_id),
    CONSTRAINT product_watchlist_state_check CHECK (state IN ('ACTIVE', 'INACTIVE_BY_USER', 'INACTIVE_BY_RESTRICTED_PLAN'))
);

CREATE INDEX product_watchlist_user_created_idx ON product_watchlist (user_id, created DESC);
CREATE INDEX product_watchlist_user_created_product_idx ON product_watchlist (user_id, created DESC, product_id ASC);
CREATE INDEX product_watchlist_product_id_idx ON product_watchlist (product_id);

CREATE TABLE search_filters (
    user_search_filter_id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    name text NOT NULL,
    notifications boolean NOT NULL DEFAULT true,
    state text NOT NULL,
    search jsonb NOT NULL,
    enhanced_search_description text,
    embedding real[],
    language text NOT NULL,
    currency text NOT NULL,
    last_hybrid_search_matched timestamptz NOT NULL DEFAULT '1970-01-01T00:00:00Z',
    version bigint NOT NULL DEFAULT 1,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT search_filters_user_id_unique UNIQUE (user_search_filter_id, user_id),
    CONSTRAINT search_filters_state_check CHECK (state IN ('ACTIVE', 'INACTIVE_BY_USER', 'INACTIVE_BY_RESTRICTED_PLAN')),
    CONSTRAINT search_filters_search_object CHECK (jsonb_typeof(search) = 'object'),
    CONSTRAINT search_filters_language_check CHECK (language IN ('de', 'en', 'fr', 'es', 'it', 'zh', 'pt', 'pl', 'tr', 'nl', 'cs', 'ja', 'ru', 'ar')),
    CONSTRAINT search_filters_currency_check CHECK (currency IN ('EUR', 'GBP', 'USD', 'AUD', 'CAD', 'NZD', 'CNY', 'BRL', 'PLN', 'TRY', 'JPY', 'CZK', 'RUB', 'AED', 'SAR', 'HKD', 'SGD', 'CHF')),
    CONSTRAINT search_filters_embedding_dimension_check CHECK (embedding IS NULL OR (array_ndims(embedding) = 1 AND cardinality(embedding) = 768)),
    CONSTRAINT search_filters_version_positive CHECK (version >= 1)
);

CREATE INDEX search_filters_user_created_idx ON search_filters (user_id, created DESC);
CREATE INDEX search_filters_state_updated_idx ON search_filters (state, updated DESC);

ALTER TABLE search_filters REPLICA IDENTITY FULL;

CREATE TABLE search_filter_matches (
    user_id uuid NOT NULL,
    user_search_filter_id uuid NOT NULL,
    product_id uuid NOT NULL REFERENCES products(product_id) ON DELETE CASCADE,
    origin_event_id uuid NOT NULL,
    price_valuation_basis text,
    price_fx_rate_id uuid REFERENCES fx_rates(fx_rate_id) ON DELETE RESTRICT,
    user_search_filter_name text,
    enhanced_match_reason text,
    feedback boolean,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_search_filter_id, product_id),
    CONSTRAINT search_filter_matches_filter_owner_fkey
        FOREIGN KEY (user_search_filter_id, user_id)
        REFERENCES search_filters(user_search_filter_id, user_id)
        ON DELETE CASCADE,
    CONSTRAINT search_filter_matches_origin_event_product_fkey
        FOREIGN KEY (product_id, origin_event_id)
        REFERENCES product_events(product_id, event_id),
    CONSTRAINT search_filter_matches_price_valuation_check CHECK (
        (price_valuation_basis IS NULL AND price_fx_rate_id IS NULL)
        OR (
            price_valuation_basis IN ('EVENT', 'SALE')
            AND price_fx_rate_id IS NOT NULL
        )
    )
);

CREATE INDEX search_filter_matches_user_created_idx ON search_filter_matches (user_id, created DESC);
CREATE INDEX search_filter_matches_user_filter_created_idx ON search_filter_matches (user_id, user_search_filter_id, created DESC);
CREATE INDEX search_filter_matches_filter_created_product_idx ON search_filter_matches (user_search_filter_id, created ASC, product_id ASC);
CREATE INDEX search_filter_matches_user_product_created_idx ON search_filter_matches (user_id, product_id, created ASC, user_search_filter_id ASC);
CREATE INDEX search_filter_matches_user_created_rank_idx ON search_filter_matches (user_id, created ASC, user_search_filter_id ASC, product_id ASC);
CREATE INDEX search_filter_matches_product_id_idx ON search_filter_matches (product_id);
CREATE INDEX search_filter_matches_origin_event_id_idx ON search_filter_matches (origin_event_id);
