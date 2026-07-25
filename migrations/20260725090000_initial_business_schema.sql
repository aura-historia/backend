CREATE TABLE IF NOT EXISTS users (
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
    created_by text NOT NULL,
    updated_by text NOT NULL,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT users_geo_lat_range CHECK (geo_address_lat IS NULL OR geo_address_lat BETWEEN -90 AND 90),
    CONSTRAINT users_geo_lon_range CHECK (geo_address_lon IS NULL OR geo_address_lon BETWEEN -180 AND 180)
);

CREATE TABLE IF NOT EXISTS user_partner_shops (
    user_id uuid NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    shop_id uuid NOT NULL,
    created timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, shop_id)
);

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM information_schema.tables
        WHERE table_schema = 'public' AND table_name = 'shops'
    ) AND NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'user_partner_shops_shop_id_fkey'
    ) THEN
        ALTER TABLE user_partner_shops
            ADD CONSTRAINT user_partner_shops_shop_id_fkey
            FOREIGN KEY (shop_id) REFERENCES shops(shop_id) ON DELETE CASCADE;
    END IF;
END $$;

CREATE INDEX IF NOT EXISTS users_created_idx ON users (created DESC);
CREATE INDEX IF NOT EXISTS user_partner_shops_shop_id_idx ON user_partner_shops (shop_id);


CREATE TABLE IF NOT EXISTS shops (
    shop_id uuid PRIMARY KEY,
    shop_slug_id text NOT NULL UNIQUE,
    name text NOT NULL,
    shop_type text NOT NULL,
    partner_status text NOT NULL,
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
    created_by text NOT NULL,
    updated_by text NOT NULL,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT shops_geo_lat_range CHECK (geo_address_lat IS NULL OR geo_address_lat BETWEEN -90 AND 90),
    CONSTRAINT shops_geo_lon_range CHECK (geo_address_lon IS NULL OR geo_address_lon BETWEEN -180 AND 180),
    CONSTRAINT shops_affiliate_configuration_object CHECK (
        affiliate_configuration IS NULL OR jsonb_typeof(affiliate_configuration) = 'object'
    )
);

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM information_schema.tables
        WHERE table_schema = 'public' AND table_name = 'user_partner_shops'
    ) AND NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'user_partner_shops_shop_id_fkey'
    ) THEN
        ALTER TABLE user_partner_shops
            ADD CONSTRAINT user_partner_shops_shop_id_fkey
            FOREIGN KEY (shop_id) REFERENCES shops(shop_id) ON DELETE CASCADE;
    END IF;
END $$;

CREATE INDEX IF NOT EXISTS shops_shop_domains_gin_idx ON shops USING gin (shop_domains);
CREATE INDEX IF NOT EXISTS shops_partner_status_updated_idx ON shops (partner_status, updated DESC);


CREATE TABLE IF NOT EXISTS partner_shop_applications (
    partner_shop_application_id uuid PRIMARY KEY,
    applicant_user_id uuid NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    business_state text NOT NULL,
    execution_state text NOT NULL,
    payload_type text NOT NULL,
    existing_shop_id uuid REFERENCES shops(shop_id) ON DELETE SET NULL,
    shop_name text,
    shop_type text,
    shop_domains text[] NOT NULL DEFAULT '{}',
    shop_url text,
    shop_image text,
    shop_structured_address_addressline text,
    shop_structured_address_addressline_extra text,
    shop_structured_address_locality text,
    shop_structured_address_region text,
    shop_structured_address_postal_code text,
    shop_structured_address_country text,
    shop_geo_address_lat double precision,
    shop_geo_address_lon double precision,
    shop_phone text,
    shop_email text,
    task_token text,
    version bigint NOT NULL DEFAULT 1,
    created_by text NOT NULL,
    updated_by text NOT NULL,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT partner_shop_applications_geo_lat_range CHECK (
        shop_geo_address_lat IS NULL OR shop_geo_address_lat BETWEEN -90 AND 90
    ),
    CONSTRAINT partner_shop_applications_geo_lon_range CHECK (
        shop_geo_address_lon IS NULL OR shop_geo_address_lon BETWEEN -180 AND 180
    )
);

CREATE INDEX IF NOT EXISTS partner_shop_applications_applicant_created_idx
    ON partner_shop_applications (applicant_user_id, created DESC);
CREATE INDEX IF NOT EXISTS partner_shop_applications_business_state_created_idx
    ON partner_shop_applications (business_state, created DESC);
CREATE INDEX IF NOT EXISTS partner_shop_applications_existing_shop_id_idx
    ON partner_shop_applications (existing_shop_id);


CREATE TABLE IF NOT EXISTS products (
    product_id uuid PRIMARY KEY,
    product_slug_id text NOT NULL,
    shop_slug_id text NOT NULL,
    seller_slug_id text NOT NULL,
    event_id uuid NOT NULL,
    shop_id uuid NOT NULL REFERENCES shops(shop_id),
    seller_id uuid NOT NULL REFERENCES shops(shop_id),
    shops_product_id text NOT NULL,
    shop_name text NOT NULL,
    seller_name text NOT NULL,
    shop_type text NOT NULL,
    structured_address_addressline text,
    structured_address_addressline_extra text,
    structured_address_locality text,
    structured_address_region text,
    structured_address_postal_code text,
    structured_address_country text,
    geo_address_lat double precision,
    geo_address_lon double precision,
    title_native_text text NOT NULL,
    title_native_language text NOT NULL,
    title_de text,
    title_en text,
    title_fr text,
    title_es text,
    title_it text,
    description_native_text text,
    description_native_language text,
    price_native_amount bigint,
    price_native_currency text,
    price_eur bigint,
    price_usd bigint,
    price_gbp bigint,
    price_aud bigint,
    price_cad bigint,
    price_nzd bigint,
    price_cny bigint,
    price_brl bigint,
    price_pln bigint,
    price_try bigint,
    price_jpy bigint,
    price_czk bigint,
    price_rub bigint,
    price_aed bigint,
    price_sar bigint,
    price_hkd bigint,
    price_sgd bigint,
    price_chf bigint,
    price_estimate_min_native_amount bigint,
    price_estimate_min_native_currency text,
    price_estimate_min_eur bigint,
    price_estimate_min_usd bigint,
    price_estimate_min_gbp bigint,
    price_estimate_min_aud bigint,
    price_estimate_min_cad bigint,
    price_estimate_min_nzd bigint,
    price_estimate_min_cny bigint,
    price_estimate_min_brl bigint,
    price_estimate_min_pln bigint,
    price_estimate_min_try bigint,
    price_estimate_min_jpy bigint,
    price_estimate_min_czk bigint,
    price_estimate_min_rub bigint,
    price_estimate_min_aed bigint,
    price_estimate_min_sar bigint,
    price_estimate_min_hkd bigint,
    price_estimate_min_sgd bigint,
    price_estimate_min_chf bigint,
    price_estimate_max_native_amount bigint,
    price_estimate_max_native_currency text,
    price_estimate_max_eur bigint,
    price_estimate_max_usd bigint,
    price_estimate_max_gbp bigint,
    price_estimate_max_aud bigint,
    price_estimate_max_cad bigint,
    price_estimate_max_nzd bigint,
    price_estimate_max_cny bigint,
    price_estimate_max_brl bigint,
    price_estimate_max_pln bigint,
    price_estimate_max_try bigint,
    price_estimate_max_jpy bigint,
    price_estimate_max_czk bigint,
    price_estimate_max_rub bigint,
    price_estimate_max_aed bigint,
    price_estimate_max_sar bigint,
    price_estimate_max_hkd bigint,
    price_estimate_max_sgd bigint,
    price_estimate_max_chf bigint,
    state text NOT NULL,
    lifecycle text NOT NULL,
    url text NOT NULL,
    view_url text NOT NULL,
    product_images jsonb NOT NULL DEFAULT '[]',
    embedding real[],
    auction_start timestamptz,
    auction_end timestamptz,
    created_by text NOT NULL,
    updated_by text NOT NULL,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT products_shop_product_unique UNIQUE (shop_id, shops_product_id),
    CONSTRAINT products_slug_unique UNIQUE (shop_slug_id, product_slug_id),
    CONSTRAINT products_geo_lat_range CHECK (geo_address_lat IS NULL OR geo_address_lat BETWEEN -90 AND 90),
    CONSTRAINT products_geo_lon_range CHECK (geo_address_lon IS NULL OR geo_address_lon BETWEEN -180 AND 180),
    CONSTRAINT products_images_array CHECK (jsonb_typeof(product_images) = 'array')
);

CREATE TABLE IF NOT EXISTS product_events (
    event_id uuid PRIMARY KEY,
    product_id uuid NOT NULL REFERENCES products(product_id) DEFERRABLE INITIALLY DEFERRED,
    shop_id uuid NOT NULL,
    shops_product_id text NOT NULL,
    event_type text NOT NULL,
    event_group text NOT NULL,
    event_type_schema_version int NOT NULL DEFAULT 1,
    payload jsonb NOT NULL,
    event_time timestamptz NOT NULL,
    created_by text NOT NULL,
    created timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT product_events_shop_product_fkey
        FOREIGN KEY (shop_id, shops_product_id) REFERENCES products(shop_id, shops_product_id)
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT product_events_group_check CHECK (
        event_group IN ('DOMAIN', 'ENRICHMENT', 'POLICY', 'LIFECYCLE')
    ),
    CONSTRAINT product_events_payload_object CHECK (jsonb_typeof(payload) = 'object')
);

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'products_event_id_fkey'
    ) THEN
        ALTER TABLE products
            ADD CONSTRAINT products_event_id_fkey
            FOREIGN KEY (event_id) REFERENCES product_events(event_id)
            DEFERRABLE INITIALLY DEFERRED;
    END IF;
END $$;

CREATE INDEX IF NOT EXISTS products_seller_id_idx ON products (seller_id);
CREATE INDEX IF NOT EXISTS products_lifecycle_updated_idx ON products (lifecycle, updated DESC);
CREATE INDEX IF NOT EXISTS product_events_product_time_idx ON product_events (product_id, event_time ASC);
CREATE INDEX IF NOT EXISTS product_events_shop_product_time_idx
    ON product_events (shop_id, shops_product_id, event_time ASC);
CREATE INDEX IF NOT EXISTS product_events_type_time_idx ON product_events (event_type, event_time ASC);


CREATE TABLE IF NOT EXISTS product_watchlist (
    user_id uuid NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    product_id uuid NOT NULL REFERENCES products(product_id) ON DELETE CASCADE,
    shop_id uuid NOT NULL,
    shops_product_id text NOT NULL,
    notifications boolean NOT NULL DEFAULT true,
    state text NOT NULL,
    created_by text NOT NULL,
    updated_by text NOT NULL,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, product_id),
    CONSTRAINT product_watchlist_user_shop_product_unique UNIQUE (user_id, shop_id, shops_product_id)
);

CREATE INDEX IF NOT EXISTS product_watchlist_user_created_idx ON product_watchlist (user_id, created DESC);
CREATE INDEX IF NOT EXISTS product_watchlist_product_id_idx ON product_watchlist (product_id);


CREATE TABLE IF NOT EXISTS search_filters (
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
    created_by text NOT NULL,
    updated_by text NOT NULL,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT search_filters_search_object CHECK (jsonb_typeof(search) = 'object')
);

CREATE INDEX IF NOT EXISTS search_filters_user_created_idx ON search_filters (user_id, created DESC);
CREATE INDEX IF NOT EXISTS search_filters_state_updated_idx ON search_filters (state, updated DESC);

CREATE TABLE IF NOT EXISTS search_filter_matches (
    user_id uuid NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    user_search_filter_id uuid NOT NULL REFERENCES search_filters(user_search_filter_id) ON DELETE CASCADE,
    product_id uuid NOT NULL REFERENCES products(product_id) ON DELETE CASCADE,
    shop_id uuid NOT NULL,
    shops_product_id text NOT NULL,
    origin_event_id uuid NOT NULL REFERENCES product_events(event_id),
    user_search_filter_name text,
    enhanced_match_reason text,
    feedback boolean,
    created_by text NOT NULL,
    updated_by text NOT NULL,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_search_filter_id, product_id),
    CONSTRAINT search_filter_matches_user_filter_shop_product_unique
        UNIQUE (user_id, user_search_filter_id, shop_id, shops_product_id)
);

CREATE INDEX IF NOT EXISTS search_filter_matches_user_created_idx
    ON search_filter_matches (user_id, created DESC);
CREATE INDEX IF NOT EXISTS search_filter_matches_user_filter_created_idx
    ON search_filter_matches (user_id, user_search_filter_id, created DESC);
CREATE INDEX IF NOT EXISTS search_filter_matches_product_id_idx ON search_filter_matches (product_id);
CREATE INDEX IF NOT EXISTS search_filter_matches_origin_event_id_idx ON search_filter_matches (origin_event_id);
