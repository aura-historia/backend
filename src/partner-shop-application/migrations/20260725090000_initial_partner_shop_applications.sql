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
