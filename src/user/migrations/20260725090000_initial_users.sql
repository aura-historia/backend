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
