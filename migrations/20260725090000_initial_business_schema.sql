-- Complete business schema for a fresh database; bootstrap requirements: docs/infra.md.

CREATE EXTENSION IF NOT EXISTS pg_trgm WITH SCHEMA public;

CREATE EXTENSION IF NOT EXISTS unaccent WITH SCHEMA public;

CREATE SCHEMA IF NOT EXISTS dms_heartbeat;

CREATE TABLE users (
    user_id uuid PRIMARY KEY,
    email text NOT NULL UNIQUE,
    first_name text,
    last_name text,
    language text,
    currency text,
    measurement_unit text,
    show_unassessed_or_sensitive_content boolean NOT NULL DEFAULT false,
    suspended boolean NOT NULL DEFAULT false,
    tier text NOT NULL,
    role text NOT NULL,
    stripe_customer_id text UNIQUE,
    version bigint NOT NULL DEFAULT 1,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    marketing_email_consent boolean NOT NULL DEFAULT false,
    marketing_email_consent_revision bigint NOT NULL DEFAULT 0,
    marketing_email_consent_changed_at timestamptz,
    CONSTRAINT users_language_check CHECK (language IS NULL OR language IN ('de', 'en', 'fr', 'es', 'it', 'zh', 'pt', 'pl', 'tr', 'nl', 'cs', 'ja', 'ru', 'ar')),
    CONSTRAINT users_measurement_unit_check CHECK (measurement_unit IS NULL OR measurement_unit IN ('METRIC', 'IMPERIAL')),
    CONSTRAINT users_tier_check CHECK (tier IN ('FREE', 'PRO', 'ULTIMATE')),
    CONSTRAINT users_role_check CHECK (role IN ('USER', 'ADMIN')),
    CONSTRAINT users_version_positive CHECK (version >= 1),
    CONSTRAINT users_marketing_email_consent_revision_nonnegative
        CHECK (marketing_email_consent_revision >= 0),
    CONSTRAINT users_currency_check CHECK (currency IS NULL OR currency IN ('EUR', 'GBP', 'USD', 'AUD', 'CAD', 'NZD', 'CNY', 'BRL', 'PLN', 'TRY', 'JPY', 'CZK', 'RUB', 'AED', 'SAR', 'HKD', 'SGD', 'CHF', 'ZAR', 'SEK', 'DKK', 'NOK', 'KRW', 'INR', 'TWD', 'HUF', 'RON', 'MXN', 'THB', 'ARS'))
);

CREATE TABLE user_cognito_identities (
    issuer text NOT NULL,
    subject text NOT NULL,
    user_id uuid NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    created timestamptz NOT NULL DEFAULT now(),
    sessions_revoked_before timestamptz,
    PRIMARY KEY (issuer, subject),
    CONSTRAINT user_cognito_identities_user_unique UNIQUE (user_id),
    CONSTRAINT user_cognito_identities_issuer_length CHECK (octet_length(issuer) BETWEEN 1 AND 2048),
    CONSTRAINT user_cognito_identities_subject_length CHECK (octet_length(subject) BETWEEN 1 AND 2048),
    CONSTRAINT user_cognito_identities_issuer_no_control CHECK (issuer !~ '[[:cntrl:]]'),
    CONSTRAINT user_cognito_identities_subject_no_control CHECK (subject !~ '[[:cntrl:]]'),
    CONSTRAINT user_cognito_identities_session_cutoff_valid CHECK (
        sessions_revoked_before IS NULL OR (
            isfinite(sessions_revoked_before)
            AND sessions_revoked_before >= TIMESTAMPTZ '1970-01-01 00:00:00+00'
        )
    )
);

CREATE TABLE parties (
    party_id uuid PRIMARY KEY,
    party_slug_id text NOT NULL,
    name text NOT NULL,
    name_search text COLLATE "C" NOT NULL,
    phone text,
    email text,
    version bigint NOT NULL DEFAULT 1,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT parties_slug_unique UNIQUE (party_slug_id),
    CONSTRAINT parties_party_slug_id_format CHECK (party_slug_id ~ '^[a-z0-9]+(-[a-z0-9]+)*$'),
    CONSTRAINT parties_name_nonblank CHECK (length(trim(name)) > 0),
    CONSTRAINT parties_name_max_bytes CHECK (octet_length(name) <= 255),
    CONSTRAINT parties_version_positive CHECK (version >= 1)
);

CREATE TABLE listing_sources (
    listing_source_id uuid PRIMARY KEY,
    listing_source_slug_id text NOT NULL,
    name text NOT NULL,
    name_search text COLLATE "C" NOT NULL,
    operator_party_id uuid NOT NULL REFERENCES parties(party_id) ON DELETE RESTRICT,
    url text,
    image text,
    referral_configuration jsonb,
    version bigint NOT NULL DEFAULT 1,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT listing_sources_slug_unique UNIQUE (listing_source_slug_id),
    CONSTRAINT listing_sources_listing_source_slug_id_format CHECK (listing_source_slug_id ~ '^[a-z0-9]+(-[a-z0-9]+)*$'),
    CONSTRAINT listing_sources_name_nonblank CHECK (length(trim(name)) > 0),
    CONSTRAINT listing_sources_name_max_bytes CHECK (octet_length(name) <= 255),
    CONSTRAINT listing_sources_referral_configuration_object CHECK (referral_configuration IS NULL OR jsonb_typeof(referral_configuration) = 'object'),
    CONSTRAINT listing_sources_version_positive CHECK (version >= 1),
    CONSTRAINT listing_sources_operator_identity UNIQUE (listing_source_id, operator_party_id)
);

CREATE TABLE listing_source_ingestion_methods (
    listing_source_id uuid NOT NULL REFERENCES listing_sources(listing_source_id) ON DELETE CASCADE,
    ingestion_method text NOT NULL,
    PRIMARY KEY (listing_source_id, ingestion_method),
    CONSTRAINT listing_source_ingestion_methods_check CHECK (ingestion_method IN ('WEB_CRAWL', 'SHOPIFY', 'WOOCOMMERCE', 'PARTNER_API'))
);

CREATE TABLE listing_source_web_crawl_ingestion_configurations (
    listing_source_id uuid PRIMARY KEY REFERENCES listing_sources(listing_source_id) ON DELETE CASCADE,
    fallback_currency text,
    CONSTRAINT listing_source_web_crawl_fallback_currency_check CHECK (fallback_currency IS NULL OR fallback_currency IN ('EUR', 'GBP', 'USD', 'AUD', 'CAD', 'NZD', 'CNY', 'BRL', 'PLN', 'TRY', 'JPY', 'CZK', 'RUB', 'AED', 'SAR', 'HKD', 'SGD', 'CHF', 'ZAR', 'SEK', 'DKK', 'NOK', 'KRW', 'INR', 'TWD', 'HUF', 'RON', 'MXN', 'THB', 'ARS'))
);

CREATE TABLE listing_source_shopify_ingestion_configurations (
    listing_source_id uuid PRIMARY KEY REFERENCES listing_sources(listing_source_id) ON DELETE CASCADE,
    domain text NOT NULL,
    currency text,
    language text,
    CONSTRAINT listing_source_shopify_domain_unique UNIQUE (domain),
    CONSTRAINT listing_source_shopify_language_check CHECK (language IS NULL OR language IN ('de', 'en', 'fr', 'es', 'it', 'zh', 'pt', 'pl', 'tr', 'nl', 'cs', 'ja', 'ru', 'ar')),
    CONSTRAINT listing_source_shopify_currency_check CHECK (currency IS NULL OR currency IN ('EUR', 'GBP', 'USD', 'AUD', 'CAD', 'NZD', 'CNY', 'BRL', 'PLN', 'TRY', 'JPY', 'CZK', 'RUB', 'AED', 'SAR', 'HKD', 'SGD', 'CHF', 'ZAR', 'SEK', 'DKK', 'NOK', 'KRW', 'INR', 'TWD', 'HUF', 'RON', 'MXN', 'THB', 'ARS'))
);

CREATE TABLE listing_source_woocommerce_ingestion_configurations (
    listing_source_id uuid PRIMARY KEY REFERENCES listing_sources(listing_source_id) ON DELETE CASCADE,
    webhook_secret text NOT NULL,
    currency text,
    language text,
    CONSTRAINT listing_source_woocommerce_language_check CHECK (language IS NULL OR language IN ('de', 'en', 'fr', 'es', 'it', 'zh', 'pt', 'pl', 'tr', 'nl', 'cs', 'ja', 'ru', 'ar')),
    CONSTRAINT listing_source_woocommerce_webhook_secret_nonblank
        CHECK (webhook_secret ~ '[^[:space:]]'),
    CONSTRAINT listing_source_woocommerce_currency_check CHECK (currency IS NULL OR currency IN ('EUR', 'GBP', 'USD', 'AUD', 'CAD', 'NZD', 'CNY', 'BRL', 'PLN', 'TRY', 'JPY', 'CZK', 'RUB', 'AED', 'SAR', 'HKD', 'SGD', 'CHF', 'ZAR', 'SEK', 'DKK', 'NOK', 'KRW', 'INR', 'TWD', 'HUF', 'RON', 'MXN', 'THB', 'ARS'))
);

CREATE TABLE auctions (
    auction_id uuid PRIMARY KEY,
    listing_source_id uuid NOT NULL
        REFERENCES listing_sources(listing_source_id) ON DELETE RESTRICT,
    source_auction_id text NOT NULL,
    name_text text,
    name_language text,
    catalogue_url text,
    format text,
    bidding_opens_at timestamptz,
    live_starts_at timestamptz,
    lots_begin_closing_at timestamptz,
    scheduled_end_at timestamptz,
    reported_status text,
    reported_lot_count bigint,
    version bigint NOT NULL DEFAULT 1,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT auctions_source_key_unique UNIQUE (listing_source_id, source_auction_id),
    CONSTRAINT auctions_id_source_unique UNIQUE (auction_id, listing_source_id),
    CONSTRAINT auctions_source_auction_id_length_check
        CHECK (octet_length(source_auction_id) BETWEEN 1 AND 512),
    CONSTRAINT auctions_format_check CHECK (format IS NULL OR format IN ('LIVE', 'TIMED')),
    CONSTRAINT auctions_reported_status_check CHECK (reported_status IS NULL OR reported_status IN (
        'SCHEDULED', 'IN_PROGRESS', 'ENDED', 'POSTPONED', 'CANCELLED'
    )),
    CONSTRAINT auctions_reported_lot_count_check CHECK (reported_lot_count IS NULL OR reported_lot_count BETWEEN 0 AND 4294967295),
    CONSTRAINT auctions_version_positive CHECK (version >= 1),
    CONSTRAINT auctions_name_localization_shape_check CHECK ((name_text IS NULL) = (name_language IS NULL))
);

CREATE TABLE auction_events (
    event_id uuid PRIMARY KEY,
    auction_id uuid NOT NULL REFERENCES auctions(auction_id) ON DELETE CASCADE,
    event_type text NOT NULL,
    event_type_schema_version smallint NOT NULL,
    payload jsonb NOT NULL,
    event_time timestamptz NOT NULL,
    CONSTRAINT auction_events_type_check CHECK (event_type IN ('AUCTION_DISCOVERED', 'AUCTION_CHANGED')),
    CONSTRAINT auction_events_schema_version_check CHECK (event_type_schema_version = 1),
    CONSTRAINT auction_events_payload_object_check CHECK (jsonb_typeof(payload) = 'object')
);

CREATE TABLE product_listing_raw_streams (
    product_listing_raw_stream_id uuid PRIMARY KEY,
    listing_source_id uuid NOT NULL REFERENCES listing_sources(listing_source_id) ON DELETE RESTRICT,
    ingestion_method text NOT NULL,
    source_record_key text NOT NULL,
    source_record_key_sha256 bytea NOT NULL,
    latest_revision bigint NOT NULL,
    latest_input_sha256 bytea,
    latest_provider_source_epoch_seconds bigint,
    latest_provider_source_nanoseconds integer,
    latest_provider_source_operation text,
    latest_provider_source_ordering_state text NOT NULL DEFAULT 'NO_ORDERING',
    latest_provider_source_observation_sha256 bytea,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT product_listing_raw_streams_ingestion_method_check
        CHECK (ingestion_method IN ('WEB_CRAWL', 'SHOPIFY', 'WOOCOMMERCE')),
    CONSTRAINT product_listing_raw_streams_source_record_key_max_bytes_check
        CHECK (octet_length(source_record_key) <= 4096),
    CONSTRAINT product_listing_raw_streams_source_record_key_sha256_length_check
        CHECK (octet_length(source_record_key_sha256) = 32),
    CONSTRAINT product_listing_raw_streams_latest_revision_nonnegative_check
        CHECK (latest_revision >= 0),
    CONSTRAINT product_listing_raw_streams_latest_input_sha256_length_check
        CHECK (latest_input_sha256 IS NULL OR octet_length(latest_input_sha256) = 32),
    CONSTRAINT product_listing_raw_streams_provider_source_ordering_shape_check
        CHECK (
            (
                latest_provider_source_ordering_state = 'NO_ORDERING'
                AND latest_provider_source_epoch_seconds IS NULL
                AND latest_provider_source_nanoseconds IS NULL
                AND latest_provider_source_operation IS NULL
                AND latest_provider_source_observation_sha256 IS NULL
            )
            OR (
                latest_provider_source_ordering_state = 'KNOWN'
                AND latest_provider_source_epoch_seconds IS NOT NULL
                AND latest_provider_source_nanoseconds IS NOT NULL
                AND latest_provider_source_nanoseconds BETWEEN 0 AND 999999999
                AND latest_provider_source_operation IS NOT NULL
                AND latest_provider_source_operation IN ('UPSERT', 'DELETE')
                AND latest_provider_source_observation_sha256 IS NOT NULL
                AND octet_length(latest_provider_source_observation_sha256) = 32
            )
            OR (
                latest_provider_source_ordering_state = 'UNKNOWN_DELETE'
                AND latest_provider_source_epoch_seconds IS NULL
                AND latest_provider_source_nanoseconds IS NULL
                AND latest_provider_source_operation IS NULL
                AND latest_provider_source_observation_sha256 IS NOT NULL
                AND octet_length(latest_provider_source_observation_sha256) = 32
            )
        ),
    CONSTRAINT product_listing_raw_streams_identity_unique
        UNIQUE (listing_source_id, ingestion_method, source_record_key_sha256)
);

CREATE TABLE product_listing_raw_provider_observation_receipts (
    product_listing_raw_stream_id uuid NOT NULL
        REFERENCES product_listing_raw_streams(product_listing_raw_stream_id) ON DELETE CASCADE,
    provider_scope text NOT NULL,
    provider_delivery_id text NOT NULL,
    observation_sha256 bytea NOT NULL,
    expires_at timestamptz NOT NULL DEFAULT now() + interval '90 days',
    created timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (
        product_listing_raw_stream_id,
        provider_scope,
        provider_delivery_id
    ),
    CONSTRAINT product_listing_raw_provider_observation_receipts_provider_scope_check
        CHECK (octet_length(provider_scope) BETWEEN 1 AND 128),
    CONSTRAINT product_listing_raw_provider_observation_receipts_provider_delivery_id_check
        CHECK (octet_length(provider_delivery_id) BETWEEN 1 AND 512),
    CONSTRAINT product_listing_raw_provider_observation_receipts_observation_sha256_length_check
        CHECK (octet_length(observation_sha256) = 32)
);

CREATE TABLE product_listing_raw_revisions (
    product_listing_raw_revision_id uuid PRIMARY KEY,
    generation bigint GENERATED ALWAYS AS IDENTITY UNIQUE,
    product_listing_raw_stream_id uuid NOT NULL
        REFERENCES product_listing_raw_streams(product_listing_raw_stream_id) ON DELETE CASCADE,
    revision bigint NOT NULL,
    operation text NOT NULL,
    payload_format text NOT NULL,
    payload_schema_version smallint NOT NULL,
    raw_values_schema_version smallint NOT NULL,
    source_payload jsonb NOT NULL,
    raw_values jsonb NOT NULL,
    normalization_context jsonb NOT NULL,
    provenance jsonb NOT NULL,
    input_sha256 bytea NOT NULL,
    source_event_id text,
    source_occurred_at timestamptz,
    captured_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT product_listing_raw_revisions_stream_revision_unique
        UNIQUE (product_listing_raw_stream_id, revision),
    CONSTRAINT product_listing_raw_revisions_operation_check
        CHECK (operation IN ('UPSERT', 'DELETE')),
    CONSTRAINT product_listing_raw_revisions_payload_format_check
        CHECK (payload_format IN ('CRAWLER_EXTRACTED_PRODUCT', 'SHOPIFY_PRODUCT', 'WOOCOMMERCE_PRODUCT')),
    CONSTRAINT product_listing_raw_revisions_payload_schema_version_positive_check
        CHECK (payload_schema_version >= 1),
    CONSTRAINT product_listing_raw_revisions_raw_values_schema_version_positive_check
        CHECK (raw_values_schema_version >= 1),
    CONSTRAINT product_listing_raw_revisions_source_payload_object_check
        CHECK (jsonb_typeof(source_payload) = 'object'),
    CONSTRAINT product_listing_raw_revisions_raw_values_object_check
        CHECK (jsonb_typeof(raw_values) = 'object'),
    CONSTRAINT product_listing_raw_revisions_normalization_context_object_check
        CHECK (jsonb_typeof(normalization_context) = 'object'),
    CONSTRAINT product_listing_raw_revisions_provenance_object_check
        CHECK (jsonb_typeof(provenance) = 'object'),
    CONSTRAINT product_listing_raw_revisions_input_sha256_length_check
        CHECK (octet_length(input_sha256) = 32),
    CONSTRAINT product_listing_raw_revisions_revision_positive_check
        CHECK (revision >= 1)
);

CREATE TABLE product_listing_raw_normalization_heads (
    product_listing_raw_stream_id uuid PRIMARY KEY
        REFERENCES product_listing_raw_streams(product_listing_raw_stream_id) ON DELETE CASCADE,
    last_processed_revision bigint NOT NULL DEFAULT 0,
    product_listing_id uuid,
    source_listing_id text,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT product_listing_raw_normalization_heads_last_processed_revision_nonnegative_check
        CHECK (last_processed_revision >= 0),
    CONSTRAINT product_listing_raw_normalization_heads_binding_check
        CHECK (
            (product_listing_id IS NULL AND source_listing_id IS NULL)
            OR (product_listing_id IS NOT NULL AND source_listing_id IS NOT NULL)
        )
);

CREATE TABLE product_listing_raw_normalizations (
    product_listing_raw_revision_id uuid NOT NULL
        REFERENCES product_listing_raw_revisions(product_listing_raw_revision_id) ON DELETE CASCADE,
    product_listing_raw_stream_id uuid NOT NULL
        REFERENCES product_listing_raw_streams(product_listing_raw_stream_id) ON DELETE CASCADE,
    revision bigint NOT NULL,
    normalizer_version smallint NOT NULL,
    outcome text NOT NULL,
    product_listing_id uuid,
    product_listing_event_id uuid,
    error_code text,
    created timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (product_listing_raw_revision_id, normalizer_version),
    CONSTRAINT product_listing_raw_normalizations_revision_positive_check CHECK (revision >= 1),
    CONSTRAINT product_listing_raw_normalizations_normalizer_version_positive_check
        CHECK (normalizer_version >= 1),
    CONSTRAINT product_listing_raw_normalizations_outcome_check
        CHECK (outcome IN ('APPLIED', 'NO_CHANGE', 'IGNORED', 'REJECTED')),
    CONSTRAINT product_listing_raw_normalizations_error_code_check
        CHECK (error_code IS NULL OR octet_length(error_code) <= 128)
);

CREATE TABLE partnerships (
    partnership_id uuid PRIMARY KEY,
    party_id uuid NOT NULL UNIQUE REFERENCES parties(party_id) ON DELETE RESTRICT,
    business_state text NOT NULL DEFAULT 'ACTIVE',
    version bigint NOT NULL DEFAULT 1,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT partnerships_business_state_check CHECK (business_state IN ('ACTIVE', 'DISSOLVED')),
    CONSTRAINT partnerships_version_positive CHECK (version >= 1)
);

CREATE TABLE partnership_members (
    user_id uuid NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    partnership_id uuid NOT NULL REFERENCES partnerships(partnership_id) ON DELETE CASCADE,
    created timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, partnership_id)
);

CREATE TABLE partnership_listing_source_grants (
    partnership_id uuid NOT NULL REFERENCES partnerships(partnership_id) ON DELETE CASCADE,
    listing_source_id uuid NOT NULL REFERENCES listing_sources(listing_source_id) ON DELETE CASCADE,
    created timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (partnership_id, listing_source_id)
);

CREATE TABLE partnership_applications (
    partnership_application_id uuid PRIMARY KEY,
    applicant_user_id uuid NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    business_state text NOT NULL,
    proposal jsonb NOT NULL,
    approved_partnership_id uuid REFERENCES partnerships(partnership_id),
    approved_listing_source_id uuid REFERENCES listing_sources(listing_source_id),
    version bigint NOT NULL DEFAULT 1,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT partnership_applications_business_state_check CHECK (business_state IN ('SUBMITTED', 'IN_REVIEW', 'APPROVED', 'REJECTED', 'WITHDRAWN')),
    CONSTRAINT partnership_applications_proposal_shape_check CHECK (
        jsonb_typeof(proposal) = 'object'
        AND proposal->>'type' IN ('EXISTING_LISTING_SOURCE', 'PROPOSED_LISTING_SOURCE')
        AND (
            (
                proposal->>'type' = 'EXISTING_LISTING_SOURCE'
                AND jsonb_typeof(proposal->'listing_source_id') = 'string'
            )
            OR (
                proposal->>'type' = 'PROPOSED_LISTING_SOURCE'
                AND jsonb_typeof(proposal->'party') = 'object'
                AND jsonb_typeof(proposal->'listing_source') = 'object'
            )
        )
    ),
    CONSTRAINT partnership_applications_approval_result_check CHECK (
        (business_state = 'APPROVED'
            AND approved_partnership_id IS NOT NULL
            AND approved_listing_source_id IS NOT NULL)
        OR (business_state <> 'APPROVED'
            AND approved_partnership_id IS NULL
            AND approved_listing_source_id IS NULL)
    ),
    CONSTRAINT partnership_applications_version_positive CHECK (version >= 1)
);

CREATE TABLE fx_rates (
    fx_rate_id uuid PRIMARY KEY,
    generation bigint GENERATED ALWAYS AS IDENTITY UNIQUE,
    captured_at timestamptz NOT NULL,
    source text NOT NULL,
    source_event_id text NOT NULL UNIQUE,
    created timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE fx_rate_quotes (
    fx_rate_id uuid NOT NULL REFERENCES fx_rates(fx_rate_id) ON DELETE RESTRICT,
    currency text NOT NULL,
    units_per_eur bigint NOT NULL CHECK (units_per_eur > 0),
    PRIMARY KEY (fx_rate_id, currency),
    CONSTRAINT fx_rate_quotes_currency_check CHECK (currency IN ('EUR', 'GBP', 'USD', 'AUD', 'CAD', 'NZD', 'CNY', 'BRL', 'PLN', 'TRY', 'JPY', 'CZK', 'RUB', 'AED', 'SAR', 'HKD', 'SGD', 'CHF', 'ZAR', 'SEK', 'DKK', 'NOK', 'KRW', 'INR', 'TWD', 'HUF', 'RON', 'MXN', 'THB', 'ARS'))
);

CREATE TABLE product_listings (
    product_listing_id uuid PRIMARY KEY,
    product_listing_title_slug_id text NOT NULL,
    version bigint NOT NULL DEFAULT 1,
    current_event_id uuid NOT NULL,
    content_source_event_id uuid NOT NULL,
    embedding_source_event_id uuid NOT NULL,
    listing_source_id uuid NOT NULL
        REFERENCES listing_sources(listing_source_id)
        ON DELETE RESTRICT,
    source_listing_id text NOT NULL,
    auction_id uuid,
    lot_number text,
    catalogue_position bigint,
    lot_bidding_opens_at timestamptz,
    lot_scheduled_closes_at timestamptz,
    lot_reported_closed_at timestamptz,
    title_text text,
    title_language text,
    description_text text,
    description_language text,
    price_kind text,
    price_amount bigint,
    price_currency text,
    price_estimate_min_amount bigint,
    price_estimate_min_currency text,
    price_estimate_max_amount bigint,
    price_estimate_max_currency text,
    sale_observation_fx_rate_id uuid REFERENCES fx_rates(fx_rate_id) ON DELETE RESTRICT,
    sale_observed_at timestamptz,
    availability text,
    lifecycle text NOT NULL,
    url text NOT NULL,
    product_images jsonb NOT NULL DEFAULT '[]',
    embedding real[],
    projection_version bigint NOT NULL DEFAULT 1,
    created timestamptz NOT NULL DEFAULT date_trunc('milliseconds', now()),
    updated timestamptz NOT NULL DEFAULT date_trunc('milliseconds', now()),
    delivery_configuration jsonb,
    delivery_verified_at timestamptz,
    CONSTRAINT product_listings_listing_source_listing_unique UNIQUE (listing_source_id, source_listing_id),
    CONSTRAINT product_listings_title_slug_unique UNIQUE (product_listing_title_slug_id),
    CONSTRAINT product_listings_auction_source_fk
        FOREIGN KEY (auction_id, listing_source_id)
        REFERENCES auctions(auction_id, listing_source_id) ON DELETE RESTRICT,
    CONSTRAINT product_listings_lot_number_check CHECK (
        lot_number IS NULL OR (
            octet_length(lot_number) BETWEEN 1 AND 128
            AND lot_number !~ '(^[[:space:]]|[[:space:]]$)'
        )
    ),
    CONSTRAINT product_listings_catalogue_position_check CHECK (
        catalogue_position IS NULL OR catalogue_position BETWEEN 1 AND 4294967295
    ),
    CONSTRAINT product_listings_lot_schedule_check CHECK (
        lot_bidding_opens_at IS NULL
        OR lot_scheduled_closes_at IS NULL
        OR lot_bidding_opens_at <= lot_scheduled_closes_at
    ),
    CONSTRAINT product_listings_source_listing_id_check CHECK (
        octet_length(source_listing_id) BETWEEN 1 AND 512
        AND source_listing_id !~ '(^[[:space:]]|[[:space:]]$)'
    ),
    CONSTRAINT product_listings_title_slug_id_format CHECK (
        product_listing_title_slug_id ~ '^[a-z0-9]+(-[a-z0-9]+)*-[0-9a-f]{6}$'
    ),
    CONSTRAINT product_listings_title_slug_id_max_bytes CHECK (
        octet_length(product_listing_title_slug_id) <= 120
    ),
    CONSTRAINT product_listings_availability_check CHECK (availability IS NULL OR availability IN ('AVAILABLE', 'IN_STOCK', 'LIMITED_AVAILABILITY', 'BACK_ORDER', 'MADE_TO_ORDER', 'PRE_ORDER', 'PRE_SALE', 'UNAVAILABLE', 'RESERVED', 'OUT_OF_STOCK', 'SOLD_OUT')),
    CONSTRAINT product_listings_lifecycle_check CHECK (lifecycle IN ('ACTIVE', 'WITHDRAWN')),
    CONSTRAINT product_listings_withdrawn_availability_check CHECK (lifecycle <> 'WITHDRAWN' OR availability IS NULL),
    CONSTRAINT product_listings_title_pair_check CHECK ((title_text IS NULL) = (title_language IS NULL)),
    CONSTRAINT product_listings_description_pair_check CHECK ((description_text IS NULL) = (description_language IS NULL)),
    CONSTRAINT product_listings_title_language_check CHECK (title_language IS NULL OR title_language IN ('de', 'en', 'fr', 'es', 'it', 'zh', 'pt', 'pl', 'tr', 'nl', 'cs', 'ja', 'ru', 'ar')),
    CONSTRAINT product_listings_description_language_check CHECK (description_language IS NULL OR description_language IN ('de', 'en', 'fr', 'es', 'it', 'zh', 'pt', 'pl', 'tr', 'nl', 'cs', 'ja', 'ru', 'ar')),
    CONSTRAINT product_listings_price_shape_check CHECK (
        (price_kind IS NULL AND price_amount IS NULL AND price_currency IS NULL)
        OR (price_kind = 'MONETARY' AND price_amount IS NOT NULL AND price_currency IS NOT NULL)
        OR (price_kind = 'ON_REQUEST' AND price_amount IS NULL AND price_currency IS NULL)
    ),
    CONSTRAINT product_listings_price_kind_check CHECK (price_kind IS NULL OR price_kind IN ('MONETARY', 'ON_REQUEST')),
    CONSTRAINT product_listings_price_estimate_min_pair_check CHECK ((price_estimate_min_amount IS NULL) = (price_estimate_min_currency IS NULL)),
    CONSTRAINT product_listings_price_estimate_max_pair_check CHECK ((price_estimate_max_amount IS NULL) = (price_estimate_max_currency IS NULL)),
    CONSTRAINT product_listings_price_amount_nonnegative CHECK (price_amount IS NULL OR price_amount >= 0),
    CONSTRAINT product_listings_price_estimate_min_amount_nonnegative CHECK (price_estimate_min_amount IS NULL OR price_estimate_min_amount >= 0),
    CONSTRAINT product_listings_price_estimate_max_amount_nonnegative CHECK (price_estimate_max_amount IS NULL OR price_estimate_max_amount >= 0),
    CONSTRAINT product_listings_sale_observation_pair_check CHECK ((sale_observation_fx_rate_id IS NULL) = (sale_observed_at IS NULL)),
    CONSTRAINT product_listings_images_array CHECK (jsonb_typeof(product_images) = 'array'),
    CONSTRAINT product_listings_embedding_dimension_check CHECK (embedding IS NULL OR (array_ndims(embedding) = 1 AND cardinality(embedding) = 768)),
    CONSTRAINT product_listings_version_positive CHECK (version >= 1),
    CONSTRAINT product_listings_projection_version_positive CHECK (projection_version >= 1),
    CONSTRAINT product_listings_delivery_object
    CHECK (delivery_configuration IS NULL OR jsonb_typeof(delivery_configuration) = 'object'),
    CONSTRAINT listing_delivery_evidence_window CHECK (
        (delivery_configuration IS NULL) = (delivery_verified_at IS NULL)),
    CONSTRAINT product_listings_price_currency_check CHECK (price_currency IS NULL OR price_currency IN ('EUR', 'GBP', 'USD', 'AUD', 'CAD', 'NZD', 'CNY', 'BRL', 'PLN', 'TRY', 'JPY', 'CZK', 'RUB', 'AED', 'SAR', 'HKD', 'SGD', 'CHF', 'ZAR', 'SEK', 'DKK', 'NOK', 'KRW', 'INR', 'TWD', 'HUF', 'RON', 'MXN', 'THB', 'ARS')),
    CONSTRAINT product_listings_price_estimate_min_currency_check CHECK (price_estimate_min_currency IS NULL OR price_estimate_min_currency IN ('EUR', 'GBP', 'USD', 'AUD', 'CAD', 'NZD', 'CNY', 'BRL', 'PLN', 'TRY', 'JPY', 'CZK', 'RUB', 'AED', 'SAR', 'HKD', 'SGD', 'CHF', 'ZAR', 'SEK', 'DKK', 'NOK', 'KRW', 'INR', 'TWD', 'HUF', 'RON', 'MXN', 'THB', 'ARS')),
    CONSTRAINT product_listings_price_estimate_max_currency_check CHECK (price_estimate_max_currency IS NULL OR price_estimate_max_currency IN ('EUR', 'GBP', 'USD', 'AUD', 'CAD', 'NZD', 'CNY', 'BRL', 'PLN', 'TRY', 'JPY', 'CZK', 'RUB', 'AED', 'SAR', 'HKD', 'SGD', 'CHF', 'ZAR', 'SEK', 'DKK', 'NOK', 'KRW', 'INR', 'TWD', 'HUF', 'RON', 'MXN', 'THB', 'ARS'))
);

CREATE TABLE product_listing_translations (
    product_listing_id uuid NOT NULL REFERENCES product_listings(product_listing_id) ON DELETE CASCADE,
    source_event_id uuid NOT NULL,
    language text NOT NULL,
    title text,
    description text,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (product_listing_id, language),
    CONSTRAINT product_listing_translations_language_check CHECK (language IN ('de', 'en', 'fr', 'es', 'it', 'zh', 'pt', 'pl', 'tr', 'nl', 'cs', 'ja', 'ru', 'ar')),
    CONSTRAINT product_listing_translations_has_content CHECK (title IS NOT NULL OR description IS NOT NULL)
);

CREATE TABLE product_listing_events (
    event_id uuid PRIMARY KEY,
    product_listing_id uuid NOT NULL REFERENCES product_listings(product_listing_id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
    event_type text NOT NULL,
    event_group text NOT NULL,
    event_type_schema_version smallint NOT NULL,
    payload jsonb NOT NULL,
    event_time timestamptz NOT NULL,
    created timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT product_listing_events_product_event_unique UNIQUE (product_listing_id, event_id),
    CONSTRAINT product_listing_events_group_check CHECK (event_group IN ('DOMAIN', 'ENRICHMENT')),
    CONSTRAINT product_listing_events_schema_version_positive CHECK (event_type_schema_version >= 1),
    CONSTRAINT product_listing_events_payload_object CHECK (jsonb_typeof(payload) = 'object'),
    CONSTRAINT product_listing_events_type_group_check CHECK (
        (event_group='DOMAIN' AND event_type IN ('PRODUCT_LISTING_DISCOVERED','PRODUCT_LISTING_CHANGED')) OR
        (event_group='ENRICHMENT' AND event_type IN ('ENRICHMENT_EMBEDDED','ENRICHMENT_TRANSLATED_TITLES','ENRICHMENT_GEOGRAPHY_UPDATED','ENRICHMENT_SHIPPING_UPDATED','ENRICHMENT_CONTENT_ASSESSED'))
    )
);

CREATE TABLE product_listing_content_assessments (
    product_listing_id uuid PRIMARY KEY
        REFERENCES product_listings(product_listing_id) ON DELETE CASCADE,
    source_event_id uuid NOT NULL,
    decision text NOT NULL,
    category text,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT product_listing_content_assessments_decision_check
        CHECK (decision IN ('ALLOWED', 'REQUIRES_CONSENT')),
    CONSTRAINT product_listing_content_assessments_category_check
        CHECK (category IS NULL OR category IN ('NAZI_GERMANY')),
    CONSTRAINT product_listing_content_assessments_decision_category_check
        CHECK ((decision = 'ALLOWED' AND category IS NULL)
            OR (decision = 'REQUIRES_CONSENT' AND category IS NOT NULL)),
    CONSTRAINT product_listing_content_assessments_source_event_fkey
        FOREIGN KEY (product_listing_id, source_event_id)
        REFERENCES product_listing_events(product_listing_id, event_id)
        ON DELETE CASCADE
);

CREATE TABLE product_listing_watchlist (
    user_id uuid NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    product_listing_id uuid NOT NULL REFERENCES product_listings(product_listing_id) ON DELETE CASCADE,
    notifications boolean NOT NULL DEFAULT true,
    state text NOT NULL,
    active_since timestamptz,
    notifications_enabled_since timestamptz,
    version bigint NOT NULL DEFAULT 1,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, product_listing_id),
    CONSTRAINT product_listing_watchlist_state_check CHECK (state IN ('ACTIVE', 'INACTIVE_BY_USER', 'INACTIVE_BY_RESTRICTED_PLAN')),
    CONSTRAINT product_listing_watchlist_active_since_check
        CHECK ((state = 'ACTIVE') = (active_since IS NOT NULL)),
    CONSTRAINT product_listing_watchlist_notifications_enabled_since_check
        CHECK (notifications = (notifications_enabled_since IS NOT NULL)),
    CONSTRAINT product_listing_watchlist_version_positive
        CHECK (version >= 1)
);

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
    version bigint NOT NULL DEFAULT 1,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    matching_evidence_policy text NOT NULL,
    CONSTRAINT search_filters_user_id_unique UNIQUE (user_search_filter_id, user_id),
    CONSTRAINT search_filters_state_check CHECK (state IN ('ACTIVE', 'INACTIVE_BY_USER', 'INACTIVE_BY_RESTRICTED_PLAN')),
    CONSTRAINT search_filters_search_object CHECK (jsonb_typeof(search) = 'object'),
    CONSTRAINT search_filters_language_check CHECK (language IN ('de', 'en', 'fr', 'es', 'it', 'zh', 'pt', 'pl', 'tr', 'nl', 'cs', 'ja', 'ru', 'ar')),
    CONSTRAINT search_filters_embedding_dimension_check CHECK (embedding IS NULL OR (array_ndims(embedding) = 1 AND cardinality(embedding) = 768)),
    CONSTRAINT search_filters_enhanced_description_non_blank CHECK (enhanced_search_description IS NULL OR btrim(enhanced_search_description) <> ''),
    CONSTRAINT search_filters_version_positive CHECK (version >= 1),
    CONSTRAINT search_filters_matching_evidence_policy_check CHECK (
            matching_evidence_policy IN ('TEXT_ONLY', 'TEXT_AND_ONE_IMAGE', 'ENHANCED_TEXT_AND_FOUR_IMAGES')
        ),
    CONSTRAINT search_filters_active_enhanced_matching_check CHECK (
        state <> 'ACTIVE' OR enhanced_search_description IS NULL OR (
            matching_evidence_policy = 'ENHANCED_TEXT_AND_FOUR_IMAGES' AND embedding IS NOT NULL
        )
    ),
    CONSTRAINT search_filters_currency_check CHECK (currency IN ('EUR', 'GBP', 'USD', 'AUD', 'CAD', 'NZD', 'CNY', 'BRL', 'PLN', 'TRY', 'JPY', 'CZK', 'RUB', 'AED', 'SAR', 'HKD', 'SGD', 'CHF', 'ZAR', 'SEK', 'DKK', 'NOK', 'KRW', 'INR', 'TWD', 'HUF', 'RON', 'MXN', 'THB', 'ARS'))
);

CREATE TABLE search_filter_periodic_match_state (
    user_search_filter_id uuid PRIMARY KEY
        REFERENCES search_filters(user_search_filter_id)
        ON DELETE CASCADE,
    matched_through timestamptz NOT NULL,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE notifications (
    notification_id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    kind text NOT NULL,
    origin_event_id uuid,
    product_listing_id uuid,
    user_search_filter_id uuid,
    partner_shop_application_id uuid,
    partnership_application_id uuid,
    payload_version smallint NOT NULL DEFAULT 1,
    payload jsonb NOT NULL,
    seen boolean NOT NULL DEFAULT false,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT notifications_kind_check CHECK (
        kind IN (
            'WATCHLIST_PRICE_CHANGED',
            'WATCHLIST_AVAILABILITY_CHANGED',
            'SEARCH_FILTER_MATCH',
            'PARTNER_APPLICATION_APPROVED',
            'PARTNER_APPLICATION_REJECTED',
            'PARTNERSHIP_APPLICATION_APPROVED',
            'PARTNERSHIP_APPLICATION_REJECTED'
        )
    ),
    CONSTRAINT notifications_payload_version_positive CHECK (
        payload_version >= 1
    ),
    CONSTRAINT notifications_payload_object CHECK (
        jsonb_typeof(payload) = 'object'
    ),
    CONSTRAINT notifications_source_shape_check CHECK (
        (
            kind IN (
                'WATCHLIST_PRICE_CHANGED',
                'WATCHLIST_AVAILABILITY_CHANGED'
            )
            AND origin_event_id IS NOT NULL
            AND product_listing_id IS NOT NULL
            AND user_search_filter_id IS NULL
            AND partner_shop_application_id IS NULL
        )
        OR
        (
            kind = 'SEARCH_FILTER_MATCH'
            AND origin_event_id IS NOT NULL
            AND product_listing_id IS NOT NULL
            AND user_search_filter_id IS NOT NULL
            AND partner_shop_application_id IS NULL
        )
        OR
        (
            kind IN (
                'PARTNER_APPLICATION_APPROVED',
                'PARTNER_APPLICATION_REJECTED'
            )
            AND origin_event_id IS NULL
            AND product_listing_id IS NULL
            AND user_search_filter_id IS NULL
            AND partner_shop_application_id IS NOT NULL
            AND partnership_application_id IS NULL
        )
        OR
        (
            kind IN (
                'PARTNERSHIP_APPLICATION_APPROVED',
                'PARTNERSHIP_APPLICATION_REJECTED'
            )
            AND origin_event_id IS NULL
            AND product_listing_id IS NULL
            AND user_search_filter_id IS NULL
            AND partner_shop_application_id IS NULL
            AND partnership_application_id IS NOT NULL
        )
    )
);

CREATE TABLE access_tokens (
    access_token_id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    token_short text NOT NULL,
    token_hash text NOT NULL,
    name text NOT NULL,
    scopes text[] NOT NULL DEFAULT '{}',
    origin text NOT NULL,
    oauth_client_id uuid,
    expires_at timestamptz,
    version bigint NOT NULL DEFAULT 1,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT access_tokens_version_positive CHECK (version >= 1),
    CONSTRAINT access_tokens_origin_check CHECK (origin IN ('USER', 'OAUTH')),
    CONSTRAINT access_tokens_oauth_origin_client_check CHECK (
            (origin = 'USER' AND oauth_client_id IS NULL)
            OR (origin = 'OAUTH' AND oauth_client_id IS NOT NULL)
        ),
    CONSTRAINT access_tokens_hash_unique UNIQUE (token_short, token_hash),
    CONSTRAINT access_tokens_scopes_check CHECK (
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
    )
);

CREATE TABLE oauth_clients (
    client_id uuid PRIMARY KEY,
    client_secret_short_token text NOT NULL,
    client_secret_long_token_hash text NOT NULL,
    name text NOT NULL,
    redirect_uris text[] NOT NULL,
    tos_uri text NOT NULL,
    policy_uri text NOT NULL,
    client_uri text NOT NULL,
    logo_uri text NOT NULL,
    scopes text[] NOT NULL DEFAULT '{}',
    version bigint NOT NULL DEFAULT 1,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT oauth_clients_version_positive CHECK (version >= 1),
    CONSTRAINT oauth_clients_redirect_uris_nonempty CHECK (cardinality(redirect_uris) > 0),
    CONSTRAINT oauth_clients_redirect_uris_no_nulls CHECK (array_position(redirect_uris, NULL) IS NULL),
    CONSTRAINT oauth_clients_scopes_check CHECK (
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
    )
);

CREATE TABLE oauth_authorization_codes (
    authorization_code text PRIMARY KEY,
    client_id uuid NOT NULL REFERENCES oauth_clients(client_id) ON DELETE CASCADE,
    user_id uuid NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    redirect_uri text NOT NULL,
    scopes text[] NOT NULL DEFAULT '{}',
    code_challenge text NOT NULL,
    code_challenge_method text NOT NULL,
    expires_at timestamptz NOT NULL,
    created timestamptz NOT NULL DEFAULT now(),
    resource TEXT,
    CONSTRAINT oauth_authorization_codes_challenge_method_check
        CHECK (code_challenge_method IN ('S256')),
    CONSTRAINT oauth_authorization_codes_scopes_check CHECK (
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
    ),
    CONSTRAINT oauth_authorization_code_resource_valid CHECK (
        resource IS NULL OR resource IN (
            'https://api.aura-historia.com',
            'https://mcp.aura-historia.com/v1',
            'https://api.stage.aura-historia.com',
            'https://mcp.stage.aura-historia.com/v1'
        )
    )
);

CREATE TABLE oauth_third_party_exchange_codes (
    third_party_exchange_code text PRIMARY KEY,
    access_token_id uuid NOT NULL REFERENCES access_tokens(access_token_id) ON DELETE CASCADE,
    access_token text NOT NULL,
    access_token_expires_at timestamptz,
    scopes text[] NOT NULL DEFAULT '{}',
    expires_at timestamptz NOT NULL,
    created timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT oauth_third_party_exchange_codes_scopes_check CHECK (
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
    )
);

CREATE TABLE product_listing_command_receipts (
    command_id text PRIMARY KEY,
    submission_id text NOT NULL,
    listing_source_id uuid NOT NULL,
    operation text NOT NULL,
    semantic_fingerprint bytea NOT NULL,
    completion_code text NOT NULL,
    completed_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    CONSTRAINT product_listing_command_receipts_command_id_check
        CHECK (command_id ~ '^plic1_[0-9a-f]{64}$'),
    CONSTRAINT product_listing_command_receipts_submission_id_check
        CHECK (submission_id ~ '^plis1_[0-9a-f]{64}$'),
    CONSTRAINT product_listing_command_receipts_fingerprint_check
        CHECK (octet_length(semantic_fingerprint) = 32),
    CONSTRAINT product_listing_command_receipts_completion_code_check
        CHECK (completion_code = 'APPLIED'),
    CONSTRAINT product_listing_command_receipts_operation_check
    CHECK (operation IN ('CREATE','UPDATE','UPSERT','WITHDRAW','CAPTURE_RAW','CHANGE_DELIVERY'))
);

CREATE TABLE marketing_email_consent_sync_intents (
    intent_id uuid PRIMARY KEY,
    intent_sequence bigint GENERATED ALWAYS AS IDENTITY UNIQUE,
    source_key text NOT NULL UNIQUE,
    subject_type text NOT NULL,
    source text NOT NULL,
    user_id uuid,
    email text NOT NULL,
    recipient_key text NOT NULL,
    desired boolean NOT NULL,
    consent_revision bigint,
    changed_at timestamptz NOT NULL,
    profile_snapshot jsonb,
    status text NOT NULL DEFAULT 'PENDING',
    attempt_count integer NOT NULL DEFAULT 0,
    lease_token uuid,
    lease_expires_at timestamptz,
    completed_lease_token uuid,
    completed_at timestamptz,
    completion_status text,
    provider_contact_id text,
    last_error_code text,
    not_after timestamptz,
    created timestamptz NOT NULL DEFAULT clock_timestamp(),
    updated timestamptz NOT NULL DEFAULT clock_timestamp(),
    CONSTRAINT marketing_email_consent_sync_intents_source_key_check CHECK (octet_length(source_key) BETWEEN 1 AND 512),
    CONSTRAINT marketing_email_consent_sync_intents_email_check CHECK (octet_length(email) BETWEEN 1 AND 320 AND email !~ '[[:cntrl:]]'),
    CONSTRAINT marketing_email_consent_sync_intents_recipient_key_check CHECK (recipient_key ~ '^[0-9a-f]{64}$'),
    CONSTRAINT marketing_email_consent_sync_intents_grant_source_check CHECK (
        NOT desired OR source IN ('COGNITO_SIGNUP', 'AURA_DOUBLE_OPT_IN')
    ),
    CONSTRAINT marketing_email_consent_sync_intents_profile_check CHECK (
        profile_snapshot IS NULL OR (jsonb_typeof(profile_snapshot) = 'object' AND octet_length(profile_snapshot::text) <= 4096)
    ),
    CONSTRAINT marketing_email_consent_sync_intents_status_check CHECK (status IN ('PENDING', 'IN_PROGRESS', 'APPLIED', 'SUPERSEDED', 'BLOCKED', 'FAILED')),
    CONSTRAINT marketing_email_consent_sync_intents_attempt_check CHECK (attempt_count >= 0),
    CONSTRAINT marketing_email_consent_sync_intents_lease_check CHECK (
        (status = 'IN_PROGRESS' AND lease_token IS NOT NULL AND lease_expires_at IS NOT NULL)
        OR (status <> 'IN_PROGRESS' AND lease_token IS NULL AND lease_expires_at IS NULL)
    ),
    CONSTRAINT marketing_email_consent_sync_intents_completion_check CHECK (
        (completed_lease_token IS NULL AND completed_at IS NULL AND completion_status IS NULL)
        OR (completed_lease_token IS NOT NULL AND completed_at IS NOT NULL
            AND completion_status = status AND status IN ('APPLIED', 'SUPERSEDED', 'BLOCKED', 'FAILED'))
    ),
    CONSTRAINT marketing_email_consent_sync_intents_expiry_check CHECK (
        (desired AND not_after = changed_at + interval '7 days')
        OR (NOT desired AND not_after IS NULL)
    ),
    CONSTRAINT marketing_email_consent_sync_intents_subject_check CHECK (
        (subject_type = 'USER' AND user_id IS NOT NULL AND consent_revision > 0
            AND source IN ('COGNITO_SIGNUP', 'AURA_DOUBLE_OPT_IN', 'USER_WITHDRAWAL', 'USER_DELETION', 'PROVIDER_RACE_REPAIR'))
        OR (subject_type = 'EMAIL_ONLY' AND user_id IS NULL AND consent_revision IS NULL
            AND source IN ('AURA_DOUBLE_OPT_IN', 'EMAIL_ONLY_WITHDRAWAL', 'PROVIDER_RACE_REPAIR'))
    ),
    CONSTRAINT marketing_email_consent_sync_intents_repair_revoke_check CHECK (
        source <> 'PROVIDER_RACE_REPAIR' OR NOT desired
    )
);

CREATE TABLE newsletter_subscription_confirmations (
    confirmation_id uuid PRIMARY KEY,
    token_digest bytea NOT NULL UNIQUE,
    email text NOT NULL,
    recipient_key text NOT NULL,
    purpose text NOT NULL DEFAULT 'EMAIL_MARKETING',
    bound_user_id uuid,
    requester_user_id uuid,
    first_name text,
    last_name text,
    language text,
    currency text,
    created_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    send_attempt_status text NOT NULL DEFAULT 'NOT_ATTEMPTED',
    confirmed_at timestamptz,
    invalidated_at timestamptz,
    resulting_intent_id uuid UNIQUE REFERENCES marketing_email_consent_sync_intents(intent_id),
    CONSTRAINT newsletter_subscription_confirmations_digest_check
        CHECK (octet_length(token_digest) = 32),
    CONSTRAINT newsletter_subscription_confirmations_email_check
        CHECK (octet_length(email) BETWEEN 1 AND 320 AND email !~ '[[:cntrl:]]'),
    CONSTRAINT newsletter_subscription_confirmations_recipient_check
        CHECK (recipient_key ~ '^[0-9a-f]{64}$'),
    CONSTRAINT newsletter_subscription_confirmations_purpose_check
        CHECK (purpose = 'EMAIL_MARKETING'),
    CONSTRAINT newsletter_subscription_confirmations_profile_check
        CHECK (
            (first_name IS NULL OR char_length(first_name) <= 64)
            AND (last_name IS NULL OR char_length(last_name) <= 64)
            AND (language IS NULL OR language ~ '^[a-z]{2,3}(-[A-Z]{2})?$')
            AND (currency IS NULL OR currency ~ '^[A-Z]{3}$')
        ),
    CONSTRAINT newsletter_subscription_confirmations_ttl_check
        CHECK (expires_at = created_at + interval '24 hours'),
    CONSTRAINT newsletter_subscription_confirmations_send_status_check
        CHECK (send_attempt_status IN (
            'NOT_ATTEMPTED', 'ACCEPTED', 'DEFINITELY_REJECTED', 'ACCEPTANCE_UNKNOWN'
        )),
    CONSTRAINT newsletter_subscription_confirmations_completion_check
        CHECK (
            (confirmed_at IS NULL AND resulting_intent_id IS NULL)
            OR (confirmed_at IS NOT NULL AND resulting_intent_id IS NOT NULL AND invalidated_at IS NULL)
        )
);

CREATE TABLE loops_webhook_receipts (
    delivery_id text PRIMARY KEY,
    raw_body_sha256 bytea NOT NULL,
    provider_event_name text NOT NULL,
    event_time timestamptz NOT NULL,
    email text,
    provider_contact_id text,
    mailing_list_id text,
    disposition text NOT NULL,
    processed_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    CONSTRAINT loops_webhook_receipts_id_check
        CHECK (octet_length(delivery_id) BETWEEN 1 AND 256 AND delivery_id !~ '[[:cntrl:]]'),
    CONSTRAINT loops_webhook_receipts_digest_check
        CHECK (octet_length(raw_body_sha256) = 32),
    CONSTRAINT loops_webhook_receipts_event_name_check
        CHECK (octet_length(provider_event_name) BETWEEN 1 AND 128 AND provider_event_name !~ '[[:cntrl:]]'),
    CONSTRAINT loops_webhook_receipts_email_check
        CHECK (email IS NULL OR (octet_length(email) BETWEEN 1 AND 320 AND email !~ '[[:cntrl:]]')),
    CONSTRAINT loops_webhook_receipts_contact_check
        CHECK (provider_contact_id IS NULL OR (octet_length(provider_contact_id) BETWEEN 1 AND 256 AND provider_contact_id !~ '[[:cntrl:]]')),
    CONSTRAINT loops_webhook_receipts_list_check
        CHECK (mailing_list_id IS NULL OR (octet_length(mailing_list_id) BETWEEN 1 AND 256 AND mailing_list_id !~ '[[:cntrl:]]')),
    CONSTRAINT loops_webhook_receipts_disposition_check
        CHECK (disposition IN (
            'APPLIED_WITHDRAWAL', 'APPLIED_CONTACT_REMOVAL', 'APPLIED_COMPLAINT_BLOCK',
            'APPLIED_RESUBSCRIPTION', 'IGNORED_UNSUPPORTED_EVENT', 'IGNORED_INVALID_RECIPIENT',
            'IGNORED_UNTRUSTWORTHY_TIME', 'IGNORED_UNRELATED_LIST', 'IGNORED_API_ECHO',
            'IGNORED_HARD_BOUNCE', 'IGNORED_CONTACT_MISMATCH', 'IGNORED_STALE',
            'IGNORED_NO_REGISTERED_USER', 'IGNORED_PROVIDER_STATE'
        )),
    CONSTRAINT loops_webhook_receipts_expiry_check
        CHECK (expires_at > processed_at)
);

CREATE TABLE loops_webhook_preference_fences (
    email text PRIMARY KEY,
    latest_event_at timestamptz NOT NULL,
    provider_contact_id text NOT NULL,
    purpose_subscribed boolean NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    CONSTRAINT loops_webhook_preference_fences_email_check
        CHECK (octet_length(email) BETWEEN 1 AND 320 AND email !~ '[[:cntrl:]]'),
    CONSTRAINT loops_webhook_preference_fences_contact_check
        CHECK (octet_length(provider_contact_id) BETWEEN 1 AND 256 AND provider_contact_id !~ '[[:cntrl:]]')
);

CREATE TABLE loops_webhook_positive_delivery_tombstones (
    delivery_id text PRIMARY KEY,
    raw_body_sha256 bytea NOT NULL,
    CONSTRAINT loops_positive_delivery_id_check
        CHECK (octet_length(delivery_id) BETWEEN 1 AND 256 AND delivery_id !~ '[[:cntrl:]]'),
    CONSTRAINT loops_positive_delivery_digest_check
        CHECK (octet_length(raw_body_sha256) = 32)
);

CREATE TABLE stripe_subscription_reconciliations (
    stripe_customer_id text PRIMARY KEY,
    revision bigint NOT NULL DEFAULT 0 CHECK (revision >= 0)
);

CREATE TABLE stripe_subscription_event_receipts (
    stripe_event_id text PRIMARY KEY,
    stripe_customer_id text NOT NULL REFERENCES stripe_subscription_reconciliations(stripe_customer_id),
    stripe_subscription_id text NOT NULL,
    fingerprint bytea NOT NULL CHECK (octet_length(fingerprint) = 32),
    applied_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE party_locations (
    party_location_id uuid PRIMARY KEY,
    party_id uuid NOT NULL REFERENCES parties(party_id) ON DELETE RESTRICT,
    label text NOT NULL CHECK (octet_length(label) BETWEEN 1 AND 255 AND length(trim(label)) > 0),
    roles text[] NOT NULL DEFAULT '{}' CHECK (roles <@ ARRAY['BUSINESS_PREMISES','WAREHOUSE','REGISTERED_ADDRESS','CORRESPONDENCE']::text[] AND cardinality(roles) <= 4),
    geography jsonb,
    position jsonb,
    lifecycle text NOT NULL CHECK (lifecycle IN ('ACTIVE','RETIRED')),
    revision bigint NOT NULL DEFAULT 1 CHECK (revision > 0),
    input_revision bigint NOT NULL DEFAULT 1 CHECK (input_revision > 0 AND input_revision <= revision),
    evidence jsonb,
    created_actor text NOT NULL,
    updated_actor text NOT NULL,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT party_locations_owner_identity UNIQUE (party_location_id, party_id)
);

CREATE TABLE party_location_management_grants (
    party_id uuid NOT NULL REFERENCES parties(party_id) ON DELETE RESTRICT,
    user_id uuid NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    granted_actor text NOT NULL,
    PRIMARY KEY (party_id, user_id)
);

CREATE TABLE party_location_changes (
    party_location_id uuid NOT NULL REFERENCES party_locations(party_location_id) ON DELETE RESTRICT,
    revision bigint NOT NULL CHECK (revision > 0),
    party_id uuid NOT NULL REFERENCES parties(party_id) ON DELETE RESTRICT,
    input_revision bigint NOT NULL CHECK (input_revision > 0),
    lifecycle text NOT NULL CHECK (lifecycle IN ('ACTIVE','RETIRED')),
    actor text NOT NULL,
    occurred_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (party_location_id, revision)
);

CREATE TABLE geo_resolution_provider_controls (
    profile text PRIMARY KEY,
    is_default boolean NOT NULL DEFAULT false,
    enabled boolean NOT NULL DEFAULT false,
    storage_permitted boolean NOT NULL DEFAULT false,
    daily_limit integer NOT NULL DEFAULT 100 CHECK (daily_limit BETWEEN 1 AND 100000),
    scope_daily_limit integer NOT NULL DEFAULT 20 CHECK (scope_daily_limit BETWEEN 1 AND 100000),
    concurrency_limit integer NOT NULL DEFAULT 2 CHECK (concurrency_limit BETWEEN 1 AND 8),
    scope_concurrency_limit integer NOT NULL DEFAULT 1 CHECK (scope_concurrency_limit BETWEEN 1 AND 8),
    budget_day date NOT NULL DEFAULT (now() AT TIME ZONE 'UTC')::date,
    calls integer NOT NULL DEFAULT 0 CHECK (calls >= 0),
    consecutive_failures integer NOT NULL DEFAULT 0 CHECK (consecutive_failures >= 0),
    open_until timestamptz
);

CREATE TABLE geo_resolution_scope_budgets (
    profile text NOT NULL REFERENCES geo_resolution_provider_controls(profile),
    scope_key text NOT NULL CHECK (scope_key ~ '^[0-9a-f]{64}$'),
    budget_day date NOT NULL,
    calls integer NOT NULL CHECK (calls >= 0),
    PRIMARY KEY(profile,scope_key)
);

CREATE TABLE geo_resolutions (
    resolution_id uuid PRIMARY KEY,
    fingerprint text NOT NULL UNIQUE CHECK (fingerprint ~ '^[0-9a-f]{64}$'),
    profile text NOT NULL REFERENCES geo_resolution_provider_controls(profile),
    policy_version text NOT NULL CHECK (policy_version IN ('PRECISION_FIRST_V1','AUTOMATIC_USER_BROWSING_V1')),
    scope_key text NOT NULL CHECK (scope_key ~ '^[0-9a-f]{64}$'),
    input jsonb,
    erased boolean NOT NULL DEFAULT false,
    generation bigint NOT NULL DEFAULT 1 CHECK (generation > 0),
    status text NOT NULL DEFAULT 'PENDING' CHECK (status IN ('PENDING','ACCEPTED','AMBIGUOUS','NO_MATCH','FAILED','BLOCKED')),
    attempts integer NOT NULL DEFAULT 0 CHECK (attempts BETWEEN 0 AND 5),
    lease_token uuid,
    lease_until timestamptz,
    next_attempt_at timestamptz NOT NULL DEFAULT now(),
    evidence jsonb,
    valid_until timestamptz,
    last_failure text,
    created timestamptz NOT NULL DEFAULT now(),
    pending_since timestamptz NOT NULL DEFAULT now(),
    CHECK ((erased AND input IS NULL AND evidence IS NULL AND status='BLOCKED') OR (NOT erased AND input IS NOT NULL)),
    CHECK ((lease_token IS NULL) = (lease_until IS NULL)),
    CHECK ((evidence IS NULL) = (valid_until IS NULL)),
    CHECK ((status IN ('ACCEPTED','AMBIGUOUS','NO_MATCH')) = (evidence IS NOT NULL)),
    CHECK (status NOT IN ('FAILED','BLOCKED') OR evidence IS NULL)
);

CREATE TABLE geo_resolution_bindings (
    subject_kind text NOT NULL CHECK (subject_kind IN ('PARTY_LOCATION','USER_LOCATION')),
    subject_id uuid NOT NULL,
    resolution_id uuid NOT NULL REFERENCES geo_resolutions(resolution_id),
    input_revision bigint NOT NULL CHECK (input_revision > 0),
    guard bigint NOT NULL CHECK (guard > 0),
    selected_candidate integer CHECK (selected_candidate BETWEEN 0 AND 10),
    selected_generation bigint CHECK (selected_generation > 0),
    reviewed_actor text,
    PRIMARY KEY(subject_kind,subject_id),
    CHECK ((selected_candidate IS NULL) = (selected_generation IS NULL)),
    CHECK ((selected_candidate IS NULL) = (reviewed_actor IS NULL))
);

CREATE TABLE geo_resolution_wakeups (
    resolution_id uuid NOT NULL REFERENCES geo_resolutions(resolution_id),
    generation bigint NOT NULL CHECK (generation > 0),
    PRIMARY KEY(resolution_id,generation)
);

CREATE TABLE geo_resolution_changes (
    change_id uuid PRIMARY KEY,
    resolution_id uuid NOT NULL REFERENCES geo_resolutions(resolution_id),
    generation bigint NOT NULL CHECK (generation > 0),
    change_kind text NOT NULL CHECK (change_kind IN ('RESOLVED','INVALIDATED','REVIEWED','REQUEUED')),
    subject_kind text,
    subject_id uuid,
    occurred_at timestamptz NOT NULL DEFAULT now(),
    CHECK ((subject_kind IS NULL) = (subject_id IS NULL))
);

CREATE TABLE listing_source_location_defaults (
    listing_source_id uuid PRIMARY KEY,
    party_id uuid NOT NULL,
    party_location_id uuid,
    business_evidence jsonb,
    inventory_evidence jsonb,
    revision bigint NOT NULL CHECK (revision >= 2),
    updated_actor text NOT NULL,
    updated timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (listing_source_id, party_id) REFERENCES listing_sources(listing_source_id, operator_party_id) ON DELETE CASCADE,
    FOREIGN KEY (party_location_id, party_id) REFERENCES party_locations(party_location_id, party_id) ON DELETE RESTRICT,
    CHECK (party_location_id IS NOT NULL OR (business_evidence IS NULL AND inventory_evidence IS NULL)),
    CHECK (business_evidence IS NULL OR (jsonb_typeof(business_evidence) = 'object' AND business_evidence ?& ARRAY['reference','scope'] AND jsonb_typeof(business_evidence->'scope') = 'string' AND jsonb_typeof(business_evidence->'reference') = 'string' AND business_evidence->>'scope' = 'ASSOCIATED_BUSINESS' AND business_evidence - 'reference' - 'scope' = '{}'::jsonb)),
    CHECK (inventory_evidence IS NULL OR (jsonb_typeof(inventory_evidence) = 'object' AND inventory_evidence ?& ARRAY['reference','scope'] AND jsonb_typeof(inventory_evidence->'scope') = 'string' AND jsonb_typeof(inventory_evidence->'reference') = 'string' AND inventory_evidence->>'scope' = 'ADVERTISED_INVENTORY' AND inventory_evidence - 'reference' - 'scope' = '{}'::jsonb))
);

CREATE TABLE listing_source_location_default_changes (
    listing_source_id uuid NOT NULL,
    revision bigint NOT NULL CHECK (revision >= 2),
    party_id uuid NOT NULL,
    party_location_id uuid,
    location_revision bigint CHECK (location_revision > 0),
    business_association boolean NOT NULL,
    advertised_inventory boolean NOT NULL,
    actor text NOT NULL,
    occurred_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (listing_source_id, revision),
    CHECK ((party_location_id IS NULL) = (location_revision IS NULL)),
    CHECK (party_location_id IS NOT NULL OR (NOT business_association AND NOT advertised_inventory))
);

CREATE TABLE user_addresses (
    address_id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    label text,
    geography jsonb,
    input_revision bigint NOT NULL DEFAULT 1 CHECK (input_revision > 0),
    version bigint NOT NULL DEFAULT 1 CHECK (version > 0),
    deleted boolean NOT NULL DEFAULT false,
    CHECK (input_revision <= version),
    UNIQUE(user_id,address_id),
    CHECK ((deleted AND label IS NULL AND geography IS NULL) OR
           (NOT deleted AND label IS NOT NULL AND octet_length(label) BETWEEN 1 AND 128 AND geography IS NOT NULL))
);

CREATE TABLE user_browsing_destinations (
    user_id uuid PRIMARY KEY REFERENCES users(user_id) ON DELETE CASCADE,
    address_id uuid,
    country text,
    postal_code text,
    version bigint NOT NULL DEFAULT 1 CHECK (version > 0),
    FOREIGN KEY(user_id,address_id) REFERENCES user_addresses(user_id,address_id),
    CHECK (country IS NULL OR country ~ '^[A-Z]{2}$'),
    CHECK (postal_code IS NULL OR (country IS NOT NULL AND octet_length(postal_code) BETWEEN 1 AND 64)),
    CHECK (address_id IS NULL OR (country IS NULL AND postal_code IS NULL))
);

CREATE TABLE product_listing_geographies (
    product_listing_id uuid PRIMARY KEY REFERENCES product_listings(product_listing_id) ON DELETE CASCADE,
    dependency jsonb NOT NULL,
    geography jsonb NOT NULL,
    valid_until timestamptz,
    geography_version bigint NOT NULL CHECK (geography_version > 0)
);

CREATE TABLE listing_geography_work (
    work_id uuid PRIMARY KEY,
    dependency_kind text NOT NULL CHECK (dependency_kind IN ('SOURCE','LOCATION','RESOLUTION','LISTING','PROFILE')),
    dependency_id text NOT NULL CHECK (octet_length(dependency_id) BETWEEN 1 AND 255),
    generation bigint NOT NULL DEFAULT 1 CHECK (generation > 0),
    after_source uuid,
    after_listing uuid,
    lease_token uuid,
    lease_until timestamptz,
    pending boolean NOT NULL DEFAULT true,
    pending_since timestamptz NOT NULL DEFAULT now(),
    UNIQUE(dependency_kind,dependency_id),
    CHECK ((after_source IS NULL) = (after_listing IS NULL)),
    CHECK ((lease_token IS NULL) = (lease_until IS NULL))
);

CREATE TABLE listing_geography_wakeups (
    work_id uuid NOT NULL REFERENCES listing_geography_work(work_id),
    generation bigint NOT NULL CHECK (generation > 0),
    PRIMARY KEY(work_id,generation)
);

CREATE TABLE search_filter_matching_runs (
    window_end timestamptz PRIMARY KEY,
    status text NOT NULL DEFAULT 'RUNNING' CHECK (status IN ('RUNNING', 'COMPLETED', 'INTERRUPTED')),
    created timestamptz NOT NULL DEFAULT now(),
    recovery_deadline timestamptz NOT NULL DEFAULT now() + INTERVAL '24 hours'
);

CREATE TABLE search_filter_matches (
    user_id uuid NOT NULL,
    user_search_filter_id uuid NOT NULL,
    product_listing_id uuid NOT NULL REFERENCES product_listings(product_listing_id) ON DELETE CASCADE,
    origin_event_id uuid NOT NULL,
    price_valuation_basis text,
    price_fx_rate_id uuid REFERENCES fx_rates(fx_rate_id) ON DELETE RESTRICT,
    user_search_filter_name text,
    feedback boolean,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    matching_run_end timestamptz REFERENCES search_filter_matching_runs,
    notification_selection_complete boolean NOT NULL DEFAULT false,
    notification_recovery_after timestamptz NOT NULL DEFAULT now(),
    shipping_fx_rate_id uuid REFERENCES fx_rates(fx_rate_id) ON DELETE RESTRICT,
    shipping_fx_captured_at timestamptz,
    shipping_target_currency text,
    shipping_maximum_minor bigint,
    PRIMARY KEY (user_search_filter_id, product_listing_id),
    CONSTRAINT search_filter_matches_filter_owner_fkey
        FOREIGN KEY (user_search_filter_id, user_id)
        REFERENCES search_filters(user_search_filter_id, user_id)
        ON DELETE CASCADE,
    CONSTRAINT search_filter_matches_origin_event_product_fkey
        FOREIGN KEY (product_listing_id, origin_event_id)
        REFERENCES product_listing_events(product_listing_id, event_id),
    CONSTRAINT search_filter_matches_price_valuation_check CHECK (
        (price_valuation_basis IS NULL AND price_fx_rate_id IS NULL)
        OR (
            price_valuation_basis IN ('CURRENT', 'EVENT', 'SALE_OBSERVATION')
            AND price_fx_rate_id IS NOT NULL
        )
    ),
    CONSTRAINT search_filter_matches_shipping_fx_check CHECK (
        (shipping_fx_rate_id IS NULL AND shipping_fx_captured_at IS NULL
         AND shipping_target_currency IS NULL AND shipping_maximum_minor IS NULL)
        OR
        (shipping_fx_rate_id IS NOT NULL AND shipping_fx_captured_at IS NOT NULL
         AND shipping_target_currency IS NOT NULL AND shipping_maximum_minor IS NOT NULL
         AND shipping_maximum_minor >= 0
         AND shipping_target_currency IN (
             'EUR','GBP','USD','AUD','CAD','NZD','CNY','BRL','PLN','TRY','JPY','CZK',
             'RUB','AED','SAR','HKD','SGD','CHF','ZAR','SEK','DKK','NOK','KRW','INR',
             'TWD','HUF','RON','MXN','THB','ARS'
         ))
    )
);

CREATE TABLE search_filter_matching_run_filters (
    window_end timestamptz NOT NULL REFERENCES search_filter_matching_runs,
    user_id uuid NOT NULL REFERENCES users ON DELETE CASCADE,
    user_search_filter_id uuid NOT NULL,
    snapshot jsonb NOT NULL,
    terminal boolean NOT NULL DEFAULT false,
    failed_attempts integer NOT NULL DEFAULT 0 CHECK (failed_attempts >= 0),
    selected_listing_ids uuid[],
    selected_event_ids uuid[],
    CHECK ((selected_listing_ids IS NULL AND selected_event_ids IS NULL) OR (
        selected_listing_ids IS NOT NULL AND selected_event_ids IS NOT NULL
        AND cardinality(selected_listing_ids) = cardinality(selected_event_ids)
        AND cardinality(selected_listing_ids) <= 50
        AND array_position(selected_listing_ids, NULL) IS NULL
        AND array_position(selected_event_ids, NULL) IS NULL)),
    PRIMARY KEY (window_end, user_search_filter_id)
);

CREATE TABLE saved_search_digest_groups (
    window_end timestamptz NOT NULL REFERENCES search_filter_matching_runs,
    user_id uuid NOT NULL REFERENCES users ON DELETE CASCADE,
    finalized_at timestamptz,
    PRIMARY KEY (window_end, user_id)
);

CREATE TABLE saved_search_digests (
    digest_id uuid PRIMARY KEY,
    window_end timestamptz NOT NULL,
    user_id uuid NOT NULL,
    payload_version smallint NOT NULL DEFAULT 1 CHECK (payload_version = 1),
    created timestamptz NOT NULL DEFAULT now(),
    UNIQUE (window_end, user_id),
    FOREIGN KEY (window_end, user_id) REFERENCES saved_search_digest_groups ON DELETE CASCADE
);

CREATE TABLE notification_deliveries (
    notification_delivery_id uuid PRIMARY KEY,
    notification_id uuid
        REFERENCES notifications(notification_id)
        ON DELETE CASCADE,
    channel text NOT NULL,
    target_key text NOT NULL,
    status text NOT NULL DEFAULT 'PENDING',
    attempt_count integer NOT NULL DEFAULT 0,
    lease_token uuid,
    lease_expires_at timestamptz,
    completed_lease_token uuid,
    completed_at timestamptz,
    provider_message_id text,
    last_error_code text,
    delivered_at timestamptz,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    digest_id uuid UNIQUE REFERENCES saved_search_digests ON DELETE CASCADE,
    CONSTRAINT notification_deliveries_notification_channel_target_unique
        UNIQUE (notification_id, channel, target_key),
    CONSTRAINT notification_deliveries_channel_check CHECK (
        channel = 'EMAIL'
    ),
    CONSTRAINT notification_deliveries_target_key_nonempty_check CHECK (
        length(trim(target_key)) > 0
    ),
    CONSTRAINT notification_deliveries_status_check CHECK (
        status IN (
            'PENDING',
            'PROCESSING',
            'DELIVERED',
            'FAILED'
        )
    ),
    CONSTRAINT notification_deliveries_attempt_count_nonnegative CHECK (
        attempt_count >= 0
    ),
    CONSTRAINT notification_deliveries_lease_shape_check CHECK (
        (
            status = 'PROCESSING'
            AND lease_token IS NOT NULL
            AND lease_expires_at IS NOT NULL
        )
        OR
        (
            status <> 'PROCESSING'
            AND lease_token IS NULL
            AND lease_expires_at IS NULL
        )
    ),
    CONSTRAINT notification_deliveries_completion_shape_check CHECK (
        (completed_lease_token IS NULL AND completed_at IS NULL)
        OR
        (
            completed_lease_token IS NOT NULL
            AND completed_at IS NOT NULL
            AND status <> 'PROCESSING'
        )
    ),
    CONSTRAINT notification_deliveries_delivered_shape_check CHECK (
        (
            status = 'DELIVERED'
            AND provider_message_id IS NOT NULL
            AND delivered_at IS NOT NULL
        )
        OR
        (
            status <> 'DELIVERED'
            AND provider_message_id IS NULL
            AND delivered_at IS NULL
        )
    ),
    CONSTRAINT notification_delivery_source_check
    CHECK ((notification_id IS NOT NULL) <> (digest_id IS NOT NULL))
);

CREATE TABLE saved_search_digest_members (
    user_id uuid NOT NULL REFERENCES users ON DELETE CASCADE,
    user_search_filter_id uuid NOT NULL,
    product_listing_id uuid NOT NULL,
    origin_event_id uuid NOT NULL,
    notification_id uuid NOT NULL UNIQUE,
    matching_run_end timestamptz NOT NULL REFERENCES search_filter_matching_runs,
    digest_id uuid REFERENCES saved_search_digests ON DELETE CASCADE,
    snapshot jsonb NOT NULL CHECK (jsonb_typeof(snapshot) = 'object'),
    created timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, user_search_filter_id, product_listing_id),
    FOREIGN KEY (matching_run_end, user_id) REFERENCES saved_search_digest_groups ON DELETE CASCADE
);

CREATE TABLE shipping_profiles (
    shipping_profile_id uuid PRIMARY KEY,
    listing_source_id uuid NOT NULL,
    current_source_id uuid REFERENCES listing_sources(listing_source_id) ON DELETE SET NULL,
    management_revision bigint NOT NULL CHECK (management_revision > 0),
    terms_revision bigint NOT NULL CHECK (terms_revision > 0 AND terms_revision <= management_revision),
    lifecycle text NOT NULL CHECK (lifecycle IN ('ACTIVE','RETIRED')),
    name text NOT NULL CHECK (octet_length(name) BETWEEN 1 AND 255),
    attributed_party_id uuid REFERENCES parties(party_id),
    terms jsonb NOT NULL CHECK (jsonb_typeof(terms) = 'object'),
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    integration_method text CONSTRAINT shipping_profiles_integration_method_valid
        CHECK (integration_method IS NULL OR integration_method IN ('SHOPIFY','WOOCOMMERCE')),
    verified_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (listing_source_id, shipping_profile_id),
    CHECK (current_source_id IS NULL OR current_source_id = listing_source_id)
);

CREATE TABLE shipping_profile_evidence (
    shipping_profile_id uuid NOT NULL REFERENCES shipping_profiles(shipping_profile_id),
    management_revision bigint NOT NULL CHECK (management_revision > 0),
    evidence jsonb NOT NULL CHECK (jsonb_typeof(evidence) = 'array'),
    actor text NOT NULL,
    recorded timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (shipping_profile_id, management_revision)
);

CREATE TABLE shipping_profile_history (
    shipping_profile_id uuid NOT NULL REFERENCES shipping_profiles(shipping_profile_id),
    management_revision bigint NOT NULL CHECK (management_revision > 0),
    terms_revision bigint NOT NULL CHECK (terms_revision > 0 AND terms_revision <= management_revision),
    lifecycle text NOT NULL CHECK (lifecycle IN ('ACTIVE','RETIRED')),
    name text NOT NULL,
    attributed_party_id uuid,
    terms jsonb NOT NULL,
    PRIMARY KEY (shipping_profile_id, management_revision)
);

CREATE TABLE shipping_profile_creation_receipts (
    listing_source_id uuid NOT NULL,
    caller text NOT NULL,
    creation_key text NOT NULL CHECK (octet_length(creation_key) BETWEEN 1 AND 128),
    shipping_profile_id uuid NOT NULL REFERENCES shipping_profiles(shipping_profile_id),
    PRIMARY KEY (listing_source_id, caller, creation_key)
);

CREATE TABLE shipping_source_defaults (
    listing_source_id uuid PRIMARY KEY,
    current_source_id uuid REFERENCES listing_sources(listing_source_id) ON DELETE SET NULL,
    shipping_profile_id uuid,
    revision bigint NOT NULL CHECK (revision >= 2),
    source_deleted boolean NOT NULL DEFAULT false,
    FOREIGN KEY (listing_source_id, shipping_profile_id) REFERENCES shipping_profiles(listing_source_id, shipping_profile_id),
    CHECK (current_source_id IS NULL OR current_source_id = listing_source_id),
    CHECK (NOT source_deleted OR shipping_profile_id IS NULL)
);

CREATE TABLE shipping_profile_location_references (
    shipping_profile_id uuid NOT NULL REFERENCES shipping_profiles(shipping_profile_id),
    party_location_id uuid NOT NULL REFERENCES party_locations(party_location_id),
    applicability text NOT NULL CHECK (applicability IN ('DISPATCH','COLLECTION')),
    PRIMARY KEY (shipping_profile_id, applicability)
);

CREATE TABLE shipping_changes (
    schema_version smallint NOT NULL DEFAULT 1 CHECK (schema_version = 1),
    listing_source_id uuid NOT NULL,
    subject_kind text NOT NULL CHECK (subject_kind IN ('PROFILE','SOURCE_DEFAULT')),
    subject_id uuid NOT NULL,
    revision bigint NOT NULL CHECK (revision > 0),
    terms_revision bigint CHECK (terms_revision > 0),
    change_kind text NOT NULL CHECK (change_kind IN ('CREATED','REPLACED','RETIRED','RESTORED','EVIDENCE_CORRECTED','DEFAULT_ASSIGNED','DEFAULT_CLEARED','SOURCE_DELETED')),
    actor text NOT NULL,
    recorded timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (subject_kind, subject_id, revision),
    CHECK ((subject_kind = 'PROFILE') = (terms_revision IS NOT NULL)),
    CHECK (subject_kind <> 'SOURCE_DEFAULT' OR subject_id = listing_source_id)
);

CREATE TABLE shipping_import_authority (
    listing_source_id uuid PRIMARY KEY REFERENCES listing_sources(listing_source_id) ON DELETE CASCADE,
    revision bigint NOT NULL DEFAULT 1 CHECK(revision>0),
    changed_at timestamptz NOT NULL DEFAULT clock_timestamp()
);

CREATE TABLE shipping_import_request_fences (
    listing_source_id uuid NOT NULL,
    producer_hash bytea NOT NULL CHECK(octet_length(producer_hash)=32),
    request_hash bytea NOT NULL CHECK (octet_length(request_hash)=32),
    payload_hash bytea NOT NULL CHECK (octet_length(payload_hash)=32),
    subject_kind text NOT NULL CHECK (subject_kind IN ('PROFILE','SOURCE_DEFAULT','LISTING')),
    subject_id uuid NOT NULL,
    replay_until timestamptz NOT NULL DEFAULT now()+interval '90 days',
    PRIMARY KEY(listing_source_id,producer_hash,request_hash)
);

CREATE TABLE shipping_profile_import_state (
    shipping_profile_id uuid PRIMARY KEY,
    listing_source_id uuid NOT NULL,
    producer_hash bytea NOT NULL CHECK (octet_length(producer_hash)=32),
    object_hash bytea NOT NULL CHECK (octet_length(object_hash)=32),
    owner_hash bytea CHECK(owner_hash IS NULL OR octet_length(owner_hash)=32),
    source_revision bigint NOT NULL,
    sequence bigint NOT NULL CHECK (sequence>0),
    observation_revision bigint NOT NULL DEFAULT 0 CHECK (observation_revision>=0),
    UNIQUE(listing_source_id,producer_hash,object_hash),
    FOREIGN KEY(listing_source_id,shipping_profile_id) REFERENCES shipping_profiles(listing_source_id,shipping_profile_id)
);

CREATE TABLE shipping_default_import_state (
    listing_source_id uuid PRIMARY KEY,
    owner_hash bytea CHECK(owner_hash IS NULL OR octet_length(owner_hash)=32),
    source_revision bigint NOT NULL,
    sequence bigint NOT NULL CHECK(sequence>0),
    observation_revision bigint NOT NULL DEFAULT 0 CHECK(observation_revision>=0),
    verified_at timestamptz
);

CREATE TABLE listing_delivery_import_state (
    product_listing_id uuid NOT NULL REFERENCES product_listings(product_listing_id) ON DELETE CASCADE,
    component text NOT NULL CHECK(component IN ('SELECTION','RATE_INPUTS','SHIPPING','COLLECTION')),
    owner_hash bytea CHECK(owner_hash IS NULL OR octet_length(owner_hash)=32),
    source_revision bigint NOT NULL,
    sequence bigint NOT NULL CHECK(sequence>0),
    observation_revision bigint NOT NULL DEFAULT 0 CHECK(observation_revision>=0),
    verified_at timestamptz,
    PRIMARY KEY(product_listing_id,component)
);

CREATE TABLE shipping_validity_changes (
    listing_source_id uuid NOT NULL,
    subject_kind text NOT NULL CHECK(subject_kind IN ('PROFILE','SOURCE_DEFAULT','LISTING','SOURCE')),
    subject_id uuid NOT NULL,
    revision bigint NOT NULL CHECK(revision>0),
    change_kind text NOT NULL CHECK(change_kind IN ('OBSERVED','OWNERSHIP_CHANGED','AUTHORITY_CHANGED')),
    recorded timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY(subject_kind,subject_id,revision)
);

CREATE TABLE product_listing_shipping (
    product_listing_id uuid PRIMARY KEY REFERENCES product_listings(product_listing_id) ON DELETE CASCADE,
    dependency jsonb NOT NULL CHECK(jsonb_typeof(dependency)='object'),
    summary jsonb NOT NULL CHECK(jsonb_typeof(summary)='object'),
    projection_version bigint NOT NULL CHECK(projection_version>0)
);

CREATE TABLE listing_shipping_dependencies (
    product_listing_id uuid NOT NULL REFERENCES product_listings(product_listing_id) ON DELETE CASCADE,
    dependency_kind text NOT NULL CHECK(dependency_kind IN ('SOURCE','PROFILE','LOCATION','RESOLUTION','PROVIDER')),
    dependency_id text NOT NULL CHECK(octet_length(dependency_id) BETWEEN 1 AND 255),
    PRIMARY KEY(product_listing_id,dependency_kind,dependency_id)
);

CREATE TABLE listing_shipping_work (
    work_id uuid PRIMARY KEY,
    dependency_kind text NOT NULL CHECK(dependency_kind IN ('SOURCE','PROFILE','LISTING','LOCATION','RESOLUTION','PROVIDER')),
    dependency_id text NOT NULL CHECK(octet_length(dependency_id) BETWEEN 1 AND 255),
    generation bigint NOT NULL DEFAULT 1 CHECK(generation>0),
    after_listing uuid,
    lease_token uuid,
    lease_until timestamptz,
    pending boolean NOT NULL DEFAULT true,
    pending_since timestamptz NOT NULL DEFAULT now(),
    UNIQUE(dependency_kind,dependency_id),
    CHECK((lease_token IS NULL)=(lease_until IS NULL))
);

CREATE TABLE listing_shipping_wakeups (
    work_id uuid NOT NULL REFERENCES listing_shipping_work(work_id),
    generation bigint NOT NULL CHECK(generation>0),
    PRIMARY KEY(work_id,generation)
);

CREATE TABLE product_listing_normalization_diagnostic_cases (
    case_id uuid PRIMARY KEY,
    generation bigint GENERATED ALWAYS AS IDENTITY UNIQUE,
    entry_point text NOT NULL,
    input_sha256 bytea NOT NULL,
    listing_source_id uuid NOT NULL,
    product_listing_raw_revision_id uuid NOT NULL
        REFERENCES product_listing_raw_revisions(product_listing_raw_revision_id) ON DELETE CASCADE,
    outcome text NOT NULL,
    issues jsonb NOT NULL,
    diagnostics_version smallint NOT NULL,
    evaluator_version smallint NOT NULL,
    build text NOT NULL,
    occurrences bigint NOT NULL DEFAULT 1,
    first_recorded_at timestamptz NOT NULL DEFAULT now(),
    last_recorded_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT product_listing_normalization_diagnostic_cases_identity_unique
        UNIQUE (entry_point, input_sha256),
    CONSTRAINT product_listing_normalization_diagnostic_cases_entry_point_check
        CHECK (entry_point IN ('RAW_VALUES_NORMALIZATION_V1')),
    CONSTRAINT product_listing_normalization_diagnostic_cases_input_sha256_length_check
        CHECK (octet_length(input_sha256) = 32),
    CONSTRAINT product_listing_normalization_diagnostic_cases_outcome_check
        CHECK (outcome IN (
            'APPLIED', 'NO_CHANGE', 'REJECTED', 'SYSTEM_FAILURE',
            'EVALUATED_RESOLVED', 'EVALUATED_INVALID'
        )),
    CONSTRAINT product_listing_normalization_diagnostic_cases_issues_array_check
        CHECK (jsonb_typeof(issues) = 'array'),
    CONSTRAINT product_listing_normalization_diagnostic_cases_versions_positive_check
        CHECK (diagnostics_version >= 1 AND evaluator_version >= 1),
    CONSTRAINT product_listing_normalization_diagnostic_cases_build_length_check
        CHECK (octet_length(build) BETWEEN 1 AND 64),
    CONSTRAINT product_listing_normalization_diagnostic_cases_occurrences_positive_check
        CHECK (occurrences >= 1)
);

CREATE TABLE user_identity_deletions (
    user_id uuid PRIMARY KEY,
    issuer text NOT NULL,
    subject text NOT NULL,
    username text,
    version bigint NOT NULL DEFAULT 0 CHECK (version >= 0),
    created timestamptz NOT NULL DEFAULT now(),
    lease_expires_at timestamptz,
    completed_at timestamptz,
    UNIQUE (issuer, subject),
    CHECK (octet_length(issuer) BETWEEN 1 AND 2048 AND issuer !~ '[[:cntrl:]]'),
    CHECK (octet_length(subject) BETWEEN 1 AND 2048 AND subject !~ '[[:cntrl:]]'),
    CHECK (username IS NULL OR (char_length(username) BETWEEN 1 AND 128 AND username !~ '[[:space:][:cntrl:]]')),
    CHECK (completed_at IS NULL OR username IS NOT NULL)
);

CREATE TABLE user_identity_deletion_requests (
    user_id uuid PRIMARY KEY REFERENCES user_identity_deletions(user_id)
);

CREATE TABLE public_catalogue_overview (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    as_of timestamptz NOT NULL,
    sources_indexed bigint NOT NULL CHECK (sources_indexed >= 0),
    live_objects bigint NOT NULL CHECK (live_objects >= 0),
    countries bigint NOT NULL CHECK (countries >= 0),
    languages bigint NOT NULL CHECK (languages >= 0),
    new_objects_last_7_days bigint NOT NULL
        CHECK (new_objects_last_7_days BETWEEN 0 AND live_objects),
    CHECK (sources_indexed <= live_objects AND countries <= live_objects)
);

-- Event pointers close a reference cycle; install them after both tables exist.
ALTER TABLE product_listings
    ADD CONSTRAINT product_listings_current_event_id_same_product_fkey
    FOREIGN KEY (product_listing_id, current_event_id)
    REFERENCES product_listing_events(product_listing_id, event_id)
    DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE product_listings
    ADD CONSTRAINT product_listings_content_source_event_same_product_fkey
    FOREIGN KEY (product_listing_id, content_source_event_id)
    REFERENCES product_listing_events(product_listing_id, event_id)
    DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE product_listings
    ADD CONSTRAINT product_listings_embedding_source_event_same_product_fkey
    FOREIGN KEY (product_listing_id, embedding_source_event_id)
    REFERENCES product_listing_events(product_listing_id, event_id)
    DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE product_listing_translations
    ADD CONSTRAINT product_listing_translations_source_event_fkey
    FOREIGN KEY (product_listing_id, source_event_id)
    REFERENCES product_listing_events(product_listing_id, event_id)
    DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE access_tokens
    ADD CONSTRAINT access_tokens_oauth_client_id_fkey
    FOREIGN KEY (oauth_client_id) REFERENCES oauth_clients(client_id) ON DELETE CASCADE;

-- Search representation only. This is STABLE because the selected unaccent
-- dictionary is configurable; maintained columns and triggers avoid claiming
-- it is immutable for generated-column or expression-index use.
CREATE FUNCTION public.aura_search_name(value text)
RETURNS text
LANGUAGE sql
STABLE
PARALLEL SAFE
AS $$
    WITH unicode_whitespace(characters) AS (
        SELECT chr(9) || chr(10) || chr(11) || chr(12) || chr(13) || chr(32)
            || chr(133) || chr(160) || chr(5760)
            || chr(8192) || chr(8193) || chr(8194) || chr(8195) || chr(8196)
            || chr(8197) || chr(8198) || chr(8199) || chr(8200) || chr(8201)
            || chr(8202) || chr(8232) || chr(8233) || chr(8239) || chr(8287)
            || chr(12288)
    )
    SELECT lower(
        public.unaccent(
            'public.unaccent'::regdictionary,
            regexp_replace(
                regexp_replace(
                    normalize(value, NFC),
                    '[' || characters || ']+',
                    ' ',
                    'g'
                ),
                '^[' || characters || ']+|[' || characters || ']+$',
                '',
                'g'
            )
        )
    )
    FROM unicode_whitespace
$$;

CREATE FUNCTION public.maintain_party_name_search()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    NEW.name_search := public.aura_search_name(NEW.name);
    RETURN NEW;
END;
$$;

CREATE FUNCTION public.maintain_listing_source_name_search()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    NEW.name_search := public.aura_search_name(NEW.name);
    RETURN NEW;
END;
$$;

CREATE FUNCTION lock_existing_listing_source_proposal()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.proposal->>'type' = 'EXISTING_LISTING_SOURCE' THEN
        PERFORM 1 FROM listing_sources
        WHERE listing_source_id = (NEW.proposal->>'listing_source_id')::uuid
        FOR KEY SHARE;
        IF NOT FOUND THEN
            RAISE EXCEPTION 'existing ListingSource proposal refers to a missing ListingSource'
                USING ERRCODE = '23503',
                      CONSTRAINT = 'partnership_applications_existing_listing_source_id_fkey';
        END IF;
    END IF;
    RETURN NEW;
END;
$$;

-- Expiry remains application correctness. This bounded maintenance operation only
-- removes rows that are already logically expired. One invocation deletes at most
-- `batch_size` direct rows per target and uses SKIP LOCKED so concurrent invocations
-- share work without waiting. Returned counts are direct deletes from each named
-- target, never inferred foreign-key cascade counts. Access-token candidates have no
-- remaining exchange-code dependents, so this function cannot turn a bounded child
-- cleanup into an unbounded cascade. A scheduler invokes one call per transaction; it
-- must not loop in one transaction.
CREATE FUNCTION cleanup_expired_credentials_and_provider_receipts(batch_size integer)
RETURNS TABLE (
    access_tokens_deleted bigint,
    authorization_codes_deleted bigint,
    third_party_exchange_codes_deleted bigint,
    provider_receipts_deleted bigint
)
LANGUAGE plpgsql
AS $$
DECLARE
    cleanup_now timestamptz := statement_timestamp();
BEGIN
    IF batch_size IS NULL OR batch_size < 1 OR batch_size > 1000 THEN
        RAISE EXCEPTION 'batch_size must be a non-null integer between 1 and 1000';
    END IF;

    WITH candidates AS (
        SELECT ctid
        FROM oauth_third_party_exchange_codes
        WHERE expires_at < cleanup_now
        ORDER BY expires_at ASC
        LIMIT batch_size
        FOR UPDATE SKIP LOCKED
    ), deleted AS (
        DELETE FROM oauth_third_party_exchange_codes AS codes
        USING candidates
        WHERE codes.ctid = candidates.ctid
        RETURNING 1
    )
    SELECT count(*) INTO third_party_exchange_codes_deleted FROM deleted;

    WITH candidates AS (
        SELECT access_tokens.ctid
        FROM access_tokens
        WHERE expires_at < cleanup_now
          AND NOT EXISTS (
              SELECT 1
              FROM oauth_third_party_exchange_codes AS codes
              WHERE codes.access_token_id = access_tokens.access_token_id
          )
        ORDER BY expires_at ASC
        LIMIT batch_size
        FOR UPDATE SKIP LOCKED
    ), deleted AS (
        DELETE FROM access_tokens
        USING candidates
        WHERE access_tokens.ctid = candidates.ctid
        RETURNING 1
    )
    SELECT count(*) INTO access_tokens_deleted FROM deleted;

    WITH candidates AS (
        SELECT ctid
        FROM oauth_authorization_codes
        WHERE expires_at < cleanup_now
        ORDER BY expires_at ASC
        LIMIT batch_size
        FOR UPDATE SKIP LOCKED
    ), deleted AS (
        DELETE FROM oauth_authorization_codes AS codes
        USING candidates
        WHERE codes.ctid = candidates.ctid
        RETURNING 1
    )
    SELECT count(*) INTO authorization_codes_deleted FROM deleted;

    WITH candidates AS (
        SELECT ctid
        FROM product_listing_raw_provider_observation_receipts
        WHERE expires_at <= cleanup_now
        ORDER BY expires_at ASC
        LIMIT batch_size
        FOR UPDATE SKIP LOCKED
    ), deleted AS (
        DELETE FROM product_listing_raw_provider_observation_receipts AS receipts
        USING candidates
        WHERE receipts.ctid = candidates.ctid
        RETURNING 1
    )
    SELECT count(*) INTO provider_receipts_deleted FROM deleted;

    RETURN NEXT;
END;
$$;

CREATE FUNCTION record_loops_positive_delivery() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.provider_event_name = 'email.resubscribed' THEN
        INSERT INTO loops_webhook_positive_delivery_tombstones (delivery_id, raw_body_sha256)
        VALUES (NEW.delivery_id, NEW.raw_body_sha256)
        ON CONFLICT (delivery_id) DO NOTHING;
    END IF;
    RETURN NEW;
END
$$;

CREATE FUNCTION erase_private_address_resolution() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    old_resolution uuid;
BEGIN
    IF TG_OP='UPDATE' AND NEW.geography IS NOT DISTINCT FROM OLD.geography AND NOT NEW.deleted THEN
        RETURN NEW;
    END IF;
    SELECT resolution_id INTO old_resolution FROM geo_resolution_bindings
        WHERE subject_kind='USER_LOCATION' AND subject_id=OLD.address_id;
    IF old_resolution IS NOT NULL THEN
        -- An address update later binds and reads its replacement resolution. Match
        -- worker/reader lock order before erasure takes the previous resolution lock.
        PERFORM 1 FROM geo_resolution_provider_controls c
            JOIN geo_resolutions r USING(profile) WHERE r.resolution_id=old_resolution FOR SHARE OF c;
        PERFORM 1 FROM geo_resolutions WHERE resolution_id=old_resolution FOR NO KEY UPDATE;
        DELETE FROM geo_resolution_bindings WHERE subject_kind='USER_LOCATION' AND subject_id=OLD.address_id;
        INSERT INTO geo_resolution_changes(change_id,resolution_id,generation,change_kind,subject_kind,subject_id)
            SELECT (substr(encode(int8send((extract(epoch FROM clock_timestamp())*1000)::bigint),'hex'),5,12) || '7' || substr(replace(gen_random_uuid()::text,'-',''),14,3) || '8' || substr(replace(gen_random_uuid()::text,'-',''),18,15))::uuid,resolution_id,generation,'INVALIDATED','USER_LOCATION',OLD.address_id
            FROM geo_resolutions WHERE resolution_id=old_resolution;
        UPDATE geo_resolutions r SET
            input=NULL,evidence=NULL,valid_until=NULL,erased=true,status='BLOCKED',
            last_failure=NULL,generation=least(generation,9223372036854775806)+1,
            fingerprint=encode(sha256(uuid_send(r.resolution_id)),'hex')
            WHERE r.resolution_id=old_resolution AND NOT EXISTS
                (SELECT 1 FROM geo_resolution_bindings b WHERE b.resolution_id=r.resolution_id);
        DELETE FROM geo_resolution_wakeups WHERE resolution_id=old_resolution
            AND EXISTS(SELECT 1 FROM geo_resolutions WHERE resolution_id=old_resolution AND erased);
    END IF;
    IF TG_OP='DELETE' OR NEW.deleted THEN
        UPDATE user_browsing_destinations SET address_id=NULL,version=least(version,9223372036854775806)+1
            WHERE user_id=OLD.user_id AND address_id=OLD.address_id;
    END IF;
    IF TG_OP='DELETE' THEN RETURN OLD; END IF;
    RETURN NEW;
END;
$$;

-- Address identity and deletion tombstones are immutable.
CREATE FUNCTION guard_user_address_identity() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.address_id<>OLD.address_id OR NEW.user_id<>OLD.user_id OR (OLD.deleted AND NOT NEW.deleted) THEN
        RAISE EXCEPTION 'user address identity is immutable' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;

-- PostgreSQL 16 has no UUIDv7 generator. Use database time + random RFC4122 bits
-- only for technical wake-up identity, preserving the workspace's UUIDv7 contract.
CREATE FUNCTION listing_geography_uuid_v7() RETURNS uuid LANGUAGE sql VOLATILE AS $$
    SELECT (lpad(to_hex((extract(epoch FROM clock_timestamp())*1000)::bigint),12,'0') ||
        '7' || substr(replace(gen_random_uuid()::text,'-',''),14,3) ||
        '8' || substr(replace(gen_random_uuid()::text,'-',''),18,15))::uuid
$$;

CREATE FUNCTION enqueue_listing_geography(kind text, id text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE w uuid; g bigint;
BEGIN
    INSERT INTO listing_geography_work(work_id,dependency_kind,dependency_id)
    VALUES(listing_geography_uuid_v7(),kind,id)
    ON CONFLICT(dependency_kind,dependency_id) DO UPDATE SET
        generation=listing_geography_work.generation+1,after_source=NULL,after_listing=NULL,
        lease_token=NULL,lease_until=NULL,pending=true,
        pending_since=CASE WHEN listing_geography_work.pending THEN listing_geography_work.pending_since ELSE now() END
    RETURNING work_id,generation INTO w,g;
    INSERT INTO listing_geography_wakeups VALUES(w,g);
END $$;

CREATE FUNCTION signal_listing_geography() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_TABLE_NAME='listing_source_location_default_changes' THEN
        PERFORM enqueue_listing_geography('SOURCE',NEW.listing_source_id::text);
    ELSIF TG_TABLE_NAME='party_location_changes' THEN
        PERFORM enqueue_listing_geography('LOCATION',NEW.party_location_id::text);
    ELSIF TG_TABLE_NAME='geo_resolution_changes' THEN
        IF NEW.subject_kind='PARTY_LOCATION' THEN
            PERFORM enqueue_listing_geography('LOCATION',NEW.subject_id::text);
        ELSIF NEW.subject_kind IS NULL THEN
            PERFORM enqueue_listing_geography('RESOLUTION',NEW.resolution_id::text);
        END IF;
    ELSIF TG_TABLE_NAME='geo_resolution_provider_controls' THEN
        IF NEW.storage_permitted IS DISTINCT FROM OLD.storage_permitted THEN
            PERFORM enqueue_listing_geography('PROFILE',NEW.profile);
        END IF;
    ELSIF TG_TABLE_NAME='product_listing_events' AND NEW.event_group='DOMAIN' THEN
        PERFORM enqueue_listing_geography('LISTING',NEW.product_listing_id::text);
    END IF;
    RETURN NEW;
END $$;

-- Run matches may only produce digest email deliveries.
CREATE FUNCTION fence_saved_search_individual_email() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.notification_id IS NOT NULL AND EXISTS (
        SELECT 1 FROM notifications n JOIN search_filter_matches m
          ON (m.user_id, m.user_search_filter_id, m.product_listing_id, m.origin_event_id) =
             (n.user_id, n.user_search_filter_id, n.product_listing_id, n.origin_event_id)
        WHERE n.notification_id = NEW.notification_id AND m.matching_run_end IS NOT NULL
    ) THEN
        RAISE EXCEPTION 'saved-search run matches require digest delivery';
    END IF;
    RETURN NEW;
END;
$$;

-- Delete fences survive the source. No dependent listing is synchronously rewritten.
CREATE FUNCTION tombstone_shipping_source() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO shipping_source_defaults(listing_source_id,current_source_id,revision,source_deleted)
    VALUES(OLD.listing_source_id,OLD.listing_source_id,2,true)
    ON CONFLICT (listing_source_id) DO UPDATE SET shipping_profile_id=NULL,
        revision=shipping_source_defaults.revision+1,source_deleted=true;
    INSERT INTO shipping_changes(listing_source_id,subject_kind,subject_id,revision,change_kind,actor)
    SELECT listing_source_id,'SOURCE_DEFAULT',listing_source_id,revision,'SOURCE_DELETED','system:source-deletion'
    FROM shipping_source_defaults WHERE listing_source_id=OLD.listing_source_id;
    RETURN OLD;
END $$;

CREATE FUNCTION reject_shipping_history_mutation() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'shipping history and change signals are immutable';
END $$;

CREATE FUNCTION shipping_assertion_verification() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.terms IS DISTINCT FROM OLD.terms THEN
        NEW.verified_at := now();
    END IF;
    RETURN NEW;
END $$;

CREATE FUNCTION listing_delivery_verification() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        IF NEW.delivery_configuration IS NOT NULL THEN NEW.delivery_verified_at := now(); END IF;
    ELSIF NEW.delivery_configuration IS DISTINCT FROM OLD.delivery_configuration THEN
        NEW.delivery_verified_at := CASE WHEN NEW.delivery_configuration IS NOT NULL THEN now() END;
    END IF;
    RETURN NEW;
END $$;

CREATE FUNCTION initialize_shipping_import_authority() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 INSERT INTO shipping_import_authority(listing_source_id) VALUES(NEW.listing_source_id);
 RETURN NEW;
END $$;

-- Reads and writes use the source's current lifecycle revision. Revocation advances it
-- even if a grant/configuration is later restored to the same value.
CREATE FUNCTION shipping_import_current(source_id uuid, source_revision bigint) RETURNS boolean
LANGUAGE sql STABLE AS $$
 SELECT EXISTS(SELECT 1 FROM shipping_import_authority WHERE listing_source_id=source_id AND revision=source_revision);
$$;

CREATE FUNCTION fence_shipping_imports(source_id uuid) RETURNS void LANGUAGE plpgsql AS $$
DECLARE changed_revision bigint;
BEGIN
 UPDATE shipping_import_authority SET revision=revision+1,
  changed_at=GREATEST(clock_timestamp(),changed_at+interval '1 microsecond')
 WHERE listing_source_id=source_id AND EXISTS(SELECT 1 FROM listing_sources s WHERE s.listing_source_id=source_id)
 RETURNING revision INTO changed_revision;
 IF changed_revision IS NOT NULL THEN
  INSERT INTO shipping_validity_changes VALUES(source_id,'SOURCE',source_id,changed_revision,'AUTHORITY_CHANGED',now());
 END IF;
END $$;

CREATE FUNCTION shipping_import_source_fence() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='UPDATE' AND TG_TABLE_NAME<>'listing_sources' THEN
  IF NEW IS NOT DISTINCT FROM OLD THEN RETURN NEW; END IF;
 END IF;
 IF TG_OP='DELETE' THEN
  IF TG_TABLE_NAME='listing_sources' THEN
   INSERT INTO shipping_validity_changes SELECT OLD.listing_source_id,'SOURCE',OLD.listing_source_id,revision+1,'AUTHORITY_CHANGED',now() FROM shipping_import_authority WHERE listing_source_id=OLD.listing_source_id;
  ELSE
   PERFORM fence_shipping_imports(OLD.listing_source_id);
  END IF;
  RETURN OLD;
 END IF;
 PERFORM fence_shipping_imports(NEW.listing_source_id);
 RETURN NEW;
END $$;

CREATE FUNCTION shipping_import_member_fence() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE source_id uuid;
BEGIN
 IF TG_TABLE_NAME='partnership_members' THEN
  FOR source_id IN SELECT listing_source_id FROM partnership_listing_source_grants WHERE partnership_id=OLD.partnership_id LOOP
   PERFORM fence_shipping_imports(source_id);
  END LOOP;
 ELSE
  IF TG_OP='UPDATE' THEN
   IF NEW.suspended IS NOT DISTINCT FROM OLD.suspended AND NEW.role IS NOT DISTINCT FROM OLD.role THEN RETURN NEW; END IF;
  END IF;
  FOR source_id IN SELECT s.listing_source_id FROM listing_sources s WHERE OLD.role='ADMIN' OR EXISTS(
   SELECT 1 FROM partnership_members m JOIN partnership_listing_source_grants g USING(partnership_id)
   WHERE m.user_id=OLD.user_id AND g.listing_source_id=s.listing_source_id) LOOP
   PERFORM fence_shipping_imports(source_id);
  END LOOP;
 END IF;
 IF TG_OP='DELETE' THEN RETURN OLD; END IF;
 RETURN NEW;
END $$;

CREATE FUNCTION shipping_import_partnership_fence() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE source_id uuid;
BEGIN
 IF NEW.business_state IS NOT DISTINCT FROM OLD.business_state AND NEW.party_id IS NOT DISTINCT FROM OLD.party_id THEN RETURN NEW; END IF;
 FOR source_id IN SELECT listing_source_id FROM partnership_listing_source_grants WHERE partnership_id=OLD.partnership_id LOOP
  PERFORM fence_shipping_imports(source_id);
 END LOOP;
 RETURN NEW;
END $$;

-- Ordinary writes protect changed components immediately, including a human edit after a
-- producer read. Import completion explicitly stamps its previously checked ownership.
CREATE FUNCTION shipping_profile_manual_ownership() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.management_revision IS DISTINCT FROM OLD.management_revision THEN
  UPDATE shipping_profile_import_state SET owner_hash=NULL WHERE shipping_profile_id=NEW.shipping_profile_id;
 END IF;
 RETURN NEW;
END $$;

CREATE FUNCTION shipping_default_manual_ownership() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP = 'INSERT' OR NEW.revision IS DISTINCT FROM OLD.revision THEN
  UPDATE shipping_default_import_state SET owner_hash=NULL,verified_at=NULL WHERE listing_source_id=NEW.listing_source_id;
 END IF;
 RETURN NEW;
END $$;

CREATE FUNCTION listing_delivery_manual_ownership() RETURNS trigger LANGUAGE plpgsql AS $$
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

CREATE FUNCTION enqueue_listing_shipping(kind text,id text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE w uuid; g bigint;
BEGIN
    INSERT INTO listing_shipping_work(work_id,dependency_kind,dependency_id)
    VALUES(listing_geography_uuid_v7(),kind,id)
    ON CONFLICT(dependency_kind,dependency_id) DO UPDATE SET
        generation=listing_shipping_work.generation+1,after_listing=NULL,
        lease_token=NULL,lease_until=NULL,pending=true,
        pending_since=CASE WHEN listing_shipping_work.pending THEN listing_shipping_work.pending_since ELSE now() END
    RETURNING work_id,generation INTO w,g;
    INSERT INTO listing_shipping_wakeups VALUES(w,g);
END $$;

CREATE FUNCTION signal_listing_shipping() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_TABLE_NAME IN ('shipping_changes','shipping_validity_changes') THEN
        PERFORM enqueue_listing_shipping(CASE NEW.subject_kind WHEN 'SOURCE_DEFAULT' THEN 'SOURCE' ELSE NEW.subject_kind END,NEW.subject_id::text);
    ELSIF TG_TABLE_NAME='product_listing_events' THEN
        PERFORM enqueue_listing_shipping('LISTING',NEW.product_listing_id::text);
    ELSIF TG_TABLE_NAME='party_location_changes' THEN
        PERFORM enqueue_listing_shipping('LOCATION',NEW.party_location_id::text);
    ELSIF TG_TABLE_NAME='geo_resolution_changes' THEN
        IF NEW.subject_kind='PARTY_LOCATION' THEN
            PERFORM enqueue_listing_shipping('LOCATION',NEW.subject_id::text);
        ELSIF NEW.subject_kind IS NULL THEN
            PERFORM enqueue_listing_shipping('RESOLUTION',NEW.resolution_id::text);
        END IF;
    ELSIF TG_TABLE_NAME='geo_resolution_provider_controls' AND NEW.storage_permitted IS DISTINCT FROM OLD.storage_permitted THEN
        PERFORM enqueue_listing_shipping('PROVIDER',NEW.profile);
    END IF;
    RETURN NEW;
END $$;

-- Canonical reverse input, including currently unavailable references so restoration can
-- invalidate them. Only identities leave this function, never sites, terms or evidence.
CREATE FUNCTION listing_shipping_references(ids uuid[]) RETURNS TABLE(product_listing_id uuid,kind text,id text)
LANGUAGE sql STABLE AS $$
    WITH listings AS (
        SELECT p.product_listing_id,p.listing_source_id,p.delivery_configuration,
            CASE WHEN p.delivery_configuration IS NULL OR p.delivery_configuration#>>'{selection,kind}'='INHERIT_SOURCE_DEFAULT' THEN d.shipping_profile_id
                 WHEN p.delivery_configuration#>>'{selection,kind}'='EXPLICIT' THEN (p.delivery_configuration#>>'{selection,profile_id}')::uuid END AS profile_id
        FROM product_listings p LEFT JOIN shipping_source_defaults d USING(listing_source_id)
        WHERE p.product_listing_id=ANY(ids)
    ), sites AS (
        SELECT l.product_listing_id,r.party_location_id FROM listings l
        JOIN shipping_profile_location_references r ON r.shipping_profile_id=l.profile_id
        UNION
        SELECT l.product_listing_id,(v.value#>>'{}')::uuid FROM listings l,
            LATERAL jsonb_path_query(l.delivery_configuration,'lax $.**.party_location_id') v(value)
    ), resolutions AS (
        SELECT s.product_listing_id,b.resolution_id,r.profile FROM sites s
        JOIN geo_resolution_bindings b ON b.subject_kind='PARTY_LOCATION' AND b.subject_id=s.party_location_id
        JOIN geo_resolutions r USING(resolution_id)
    )
    SELECT product_listing_id,'SOURCE',listing_source_id::text FROM listings
    UNION SELECT product_listing_id,'PROFILE',profile_id::text FROM listings WHERE profile_id IS NOT NULL
    UNION SELECT product_listing_id,'LOCATION',party_location_id::text FROM sites
    UNION SELECT product_listing_id,'RESOLUTION',resolution_id::text FROM resolutions
    UNION SELECT product_listing_id,'PROVIDER',profile FROM resolutions
$$;

-- Entity freshness is database-owned and strictly monotonic per row. Writers compute the next
-- value atomically from the stored value after their concurrency checks, so a transaction that
-- started before a newer committed write, equal clock readings or a clock behind the stored value
-- cannot regress or repeat `updated`. Millisecond precision matches the OpenSearch `date` mapping,
-- so every advance stays distinguishable in indexed and serialized representations.
CREATE FUNCTION aura_advance_updated(previous timestamptz) RETURNS timestamptz
LANGUAGE sql VOLATILE PARALLEL SAFE AS $$
    SELECT GREATEST(
        date_trunc('milliseconds', clock_timestamp()),
        date_trunc('milliseconds', previous) + INTERVAL '1 millisecond'
    )
$$;

CREATE INDEX users_created_idx ON users (created DESC);

CREATE TRIGGER parties_maintain_name_search
BEFORE INSERT OR UPDATE OF name, name_search ON parties
FOR EACH ROW EXECUTE FUNCTION public.maintain_party_name_search();

CREATE TRIGGER listing_sources_maintain_name_search
BEFORE INSERT OR UPDATE OF name, name_search ON listing_sources
FOR EACH ROW EXECUTE FUNCTION public.maintain_listing_source_name_search();

CREATE INDEX listing_sources_operator_party_id_idx ON listing_sources (operator_party_id);

CREATE INDEX listing_sources_name_search_trgm_idx
    ON listing_sources USING gin (name_search public.gin_trgm_ops);

CREATE INDEX parties_name_search_trgm_idx
    ON parties USING gin (name_search public.gin_trgm_ops);

CREATE INDEX listing_sources_public_name_order_idx
    ON listing_sources (name_search COLLATE "C", listing_source_id);

CREATE INDEX parties_public_name_prefix_idx
    ON parties (name_search COLLATE "C", party_id);

CREATE INDEX listing_source_ingestion_methods_method_idx ON listing_source_ingestion_methods (ingestion_method, listing_source_id);

CREATE INDEX auctions_created_id_idx
    ON auctions (created DESC, auction_id DESC);

CREATE INDEX auction_events_auction_time_idx ON auction_events (auction_id, event_time, event_id);

CREATE INDEX product_listing_raw_provider_observation_receipts_expires_at_idx
    ON product_listing_raw_provider_observation_receipts (expires_at);

CREATE INDEX product_listing_raw_normalizations_stream_revision_idx
    ON product_listing_raw_normalizations (product_listing_raw_stream_id, revision ASC);

CREATE INDEX partnerships_created_id_idx
    ON partnerships (created DESC, partnership_id DESC);

CREATE INDEX partnership_members_partnership_id_idx ON partnership_members (partnership_id);

CREATE INDEX partnership_listing_source_grants_listing_source_id_idx
    ON partnership_listing_source_grants (listing_source_id, partnership_id);

CREATE INDEX partnership_applications_created_id_idx
    ON partnership_applications (created DESC, partnership_application_id DESC);

CREATE INDEX partnership_applications_updated_id_idx
    ON partnership_applications (updated DESC, partnership_application_id DESC);

CREATE INDEX partnership_applications_applicant_created_idx
    ON partnership_applications (applicant_user_id, created DESC, partnership_application_id DESC);

CREATE INDEX partnership_applications_business_state_created_idx
    ON partnership_applications (business_state, created DESC, partnership_application_id DESC);

CREATE INDEX partnership_applications_approved_source_created_id_idx
    ON partnership_applications (approved_listing_source_id, created DESC, partnership_application_id DESC)
    WHERE approved_listing_source_id IS NOT NULL;

CREATE INDEX partnership_applications_existing_source_proposal_idx
    ON partnership_applications ((proposal->>'listing_source_id'))
    WHERE proposal->>'type' = 'EXISTING_LISTING_SOURCE';

CREATE TRIGGER partnership_applications_lock_existing_listing_source
BEFORE INSERT OR UPDATE OF proposal ON partnership_applications
FOR EACH ROW EXECUTE FUNCTION lock_existing_listing_source_proposal();

CREATE INDEX fx_rates_captured_at_generation_idx
    ON fx_rates (captured_at DESC, generation DESC);

CREATE INDEX product_listings_auction_catalogue_idx
    ON product_listings (auction_id, catalogue_position, product_listing_id);

CREATE INDEX product_listings_listing_source_id_idx ON product_listings (listing_source_id);

CREATE INDEX product_listings_lifecycle_updated_idx ON product_listings (lifecycle, updated DESC);

CREATE INDEX product_listings_sale_observation_fx_rate_id_idx ON product_listings (sale_observation_fx_rate_id);

CREATE INDEX product_listing_translations_language_idx ON product_listing_translations (language);

CREATE INDEX product_listing_events_product_listing_time_event_idx
    ON product_listing_events (product_listing_id, event_time ASC, event_id ASC);

CREATE INDEX product_listing_translations_source_event_idx
    ON product_listing_translations (product_listing_id, source_event_id);

CREATE INDEX product_listing_events_type_time_idx ON product_listing_events (event_type, event_time ASC);

CREATE INDEX product_listing_watchlist_user_created_product_listing_idx
    ON product_listing_watchlist (user_id, created DESC, product_listing_id ASC);

CREATE INDEX product_listing_watchlist_product_user_idx
    ON product_listing_watchlist (product_listing_id, user_id ASC);

CREATE INDEX product_listing_watchlist_product_active_since_idx
    ON product_listing_watchlist (product_listing_id, active_since, user_id)
    WHERE state = 'ACTIVE';

CREATE INDEX search_filters_user_created_idx ON search_filters (user_id, created DESC);

CREATE INDEX search_filters_state_updated_idx ON search_filters (state, updated DESC);

CREATE INDEX search_filter_matches_user_created_idx ON search_filter_matches (user_id, created DESC);

CREATE INDEX search_filter_matches_user_filter_created_idx ON search_filter_matches (user_id, user_search_filter_id, created DESC);

CREATE INDEX search_filter_matches_filter_created_product_listing_idx
    ON search_filter_matches (user_search_filter_id, created ASC, product_listing_id ASC);

CREATE INDEX search_filter_matches_filter_created_desc_product_listing_idx
    ON search_filter_matches (user_search_filter_id, created DESC, product_listing_id ASC);

CREATE INDEX search_filter_matches_user_product_listing_created_idx ON search_filter_matches (user_id, product_listing_id, created ASC, user_search_filter_id ASC);

CREATE INDEX search_filter_matches_user_created_rank_idx ON search_filter_matches (user_id, created ASC, user_search_filter_id ASC, product_listing_id ASC);

CREATE INDEX search_filter_matches_product_listing_id_idx ON search_filter_matches (product_listing_id);

CREATE INDEX search_filter_matches_origin_event_id_idx ON search_filter_matches (origin_event_id);

CREATE UNIQUE INDEX notifications_watchlist_identity_idx
    ON notifications (user_id, origin_event_id, kind)
    WHERE kind IN (
        'WATCHLIST_PRICE_CHANGED',
        'WATCHLIST_AVAILABILITY_CHANGED'
    );

CREATE UNIQUE INDEX notifications_search_filter_identity_idx
    ON notifications (
        user_id,
        user_search_filter_id,
        product_listing_id,
        origin_event_id
    )
    WHERE kind = 'SEARCH_FILTER_MATCH';

CREATE UNIQUE INDEX notifications_partner_application_identity_idx
    ON notifications (
        user_id,
        partner_shop_application_id
    )
    WHERE kind IN (
        'PARTNER_APPLICATION_APPROVED',
        'PARTNER_APPLICATION_REJECTED'
    );

CREATE UNIQUE INDEX notifications_partnership_application_identity_idx
    ON notifications (
        user_id,
        partnership_application_id
    )
    WHERE kind IN (
        'PARTNERSHIP_APPLICATION_APPROVED',
        'PARTNERSHIP_APPLICATION_REJECTED'
    );

CREATE INDEX notifications_user_created_idx
    ON notifications (
        user_id,
        created DESC,
        notification_id DESC
    );

CREATE INDEX notifications_user_product_listing_unseen_idx
    ON notifications (
        user_id,
        product_listing_id,
        created DESC,
        notification_id DESC
    )
    WHERE seen = false
      AND product_listing_id IS NOT NULL;

CREATE INDEX notification_deliveries_status_created_idx
    ON notification_deliveries (
        status,
        created,
        notification_delivery_id
    );

CREATE INDEX access_tokens_user_created_idx
    ON access_tokens (user_id, created ASC, access_token_id ASC);

CREATE INDEX access_tokens_expires_at_idx
    ON access_tokens (expires_at)
    WHERE expires_at IS NOT NULL;

CREATE INDEX access_tokens_oauth_client_id_idx
    ON access_tokens (oauth_client_id);

CREATE INDEX oauth_authorization_codes_expires_at_idx
    ON oauth_authorization_codes (expires_at);

CREATE INDEX oauth_third_party_exchange_codes_access_token_idx
    ON oauth_third_party_exchange_codes (access_token_id);

CREATE INDEX oauth_third_party_exchange_codes_expires_at_idx
    ON oauth_third_party_exchange_codes (expires_at);

CREATE INDEX marketing_email_consent_sync_intents_claim_idx
    ON marketing_email_consent_sync_intents (intent_sequence)
    WHERE status IN ('PENDING', 'IN_PROGRESS');

CREATE INDEX marketing_email_consent_sync_intents_recipient_idx
    ON marketing_email_consent_sync_intents (recipient_key, intent_sequence DESC);

CREATE INDEX marketing_email_consent_sync_intents_user_idx
    ON marketing_email_consent_sync_intents (user_id, intent_sequence DESC) WHERE user_id IS NOT NULL;

CREATE INDEX newsletter_subscription_confirmations_recipient_created_idx
    ON newsletter_subscription_confirmations (recipient_key, email, created_at DESC);

CREATE INDEX newsletter_subscription_confirmations_expiry_idx
    ON newsletter_subscription_confirmations (expires_at, confirmation_id)
    WHERE confirmed_at IS NULL;

CREATE INDEX newsletter_subscription_confirmations_bound_user_idx
    ON newsletter_subscription_confirmations (bound_user_id)
    WHERE bound_user_id IS NOT NULL AND confirmed_at IS NULL;

CREATE INDEX loops_webhook_receipts_expiry_idx
    ON loops_webhook_receipts (expires_at, delivery_id);

-- Consent workflow cleanup indexes
-- Retention scans use completion times. Leave active/blocked/failed work outside
-- these indexes and outside automatic cleanup.
CREATE INDEX newsletter_subscription_confirmations_confirmed_cleanup_idx
    ON newsletter_subscription_confirmations (confirmed_at, confirmation_id)
    WHERE confirmed_at IS NOT NULL;

CREATE INDEX marketing_email_consent_sync_intents_completed_cleanup_idx
    ON marketing_email_consent_sync_intents (completed_at, intent_id)
    WHERE status IN ('APPLIED', 'SUPERSEDED') AND completed_at IS NOT NULL;

-- Receipt retention is measured from processing time.
CREATE INDEX loops_webhook_receipts_processed_cleanup_idx
    ON loops_webhook_receipts (processed_at, delivery_id);

-- Operator samples remain bounded without a full scan of old completed work.
CREATE INDEX marketing_email_consent_sync_intents_unsettled_diagnostic_idx
    ON marketing_email_consent_sync_intents (status, intent_sequence)
    WHERE status IN ('BLOCKED', 'FAILED');

CREATE INDEX marketing_email_consent_sync_intents_repair_diagnostic_idx
    ON marketing_email_consent_sync_intents (intent_sequence)
    WHERE source = 'PROVIDER_RACE_REPAIR' AND status NOT IN ('APPLIED', 'SUPERSEDED');

CREATE INDEX marketing_email_consent_sync_intents_race_diagnostic_idx
    ON marketing_email_consent_sync_intents (intent_sequence)
    WHERE status = 'BLOCKED' AND last_error_code = 'PROVIDER_WITHDRAWAL_RACE_CANDIDATE';

CREATE INDEX marketing_email_consent_sync_intents_expired_grant_diagnostic_idx
    ON marketing_email_consent_sync_intents (not_after, intent_id)
    WHERE desired AND status IN ('PENDING', 'IN_PROGRESS', 'BLOCKED', 'FAILED');

CREATE TRIGGER record_loops_positive_delivery
AFTER INSERT ON loops_webhook_receipts
FOR EACH ROW EXECUTE FUNCTION record_loops_positive_delivery();

CREATE INDEX party_locations_party_cursor_idx ON party_locations(party_id, party_location_id);

CREATE UNIQUE INDEX geo_resolution_default_profile_idx ON geo_resolution_provider_controls(is_default) WHERE is_default;

CREATE INDEX geo_resolution_pending_idx ON geo_resolutions(next_attempt_at,created) WHERE status='PENDING';

CREATE INDEX geo_resolution_expiry_idx ON geo_resolutions(valid_until) WHERE evidence IS NOT NULL;

CREATE INDEX geo_resolution_binding_resolution_idx ON geo_resolution_bindings(resolution_id);

CREATE INDEX listing_source_location_defaults_location_idx ON listing_source_location_defaults(party_location_id) WHERE party_location_id IS NOT NULL;

CREATE INDEX user_addresses_page_idx ON user_addresses(user_id,address_id) WHERE NOT deleted;

-- Before deletion, preferences lose their composite FK and private work is detached.
CREATE TRIGGER user_address_erasure BEFORE DELETE OR UPDATE OF geography,deleted ON user_addresses
    FOR EACH ROW EXECUTE FUNCTION erase_private_address_resolution();

CREATE TRIGGER user_address_identity BEFORE UPDATE ON user_addresses
    FOR EACH ROW EXECUTE FUNCTION guard_user_address_identity();

CREATE INDEX product_listing_geographies_expiry_idx ON product_listing_geographies(valid_until) WHERE valid_until IS NOT NULL;

CREATE INDEX product_listings_source_geography_cursor_idx ON product_listings(listing_source_id,product_listing_id);

CREATE INDEX listing_geography_work_pending_idx ON listing_geography_work(pending_since) WHERE pending;

CREATE TRIGGER listing_geography_default_signal AFTER INSERT ON listing_source_location_default_changes FOR EACH ROW EXECUTE FUNCTION signal_listing_geography();

CREATE TRIGGER listing_geography_location_signal AFTER INSERT ON party_location_changes FOR EACH ROW EXECUTE FUNCTION signal_listing_geography();

CREATE TRIGGER listing_geography_resolution_signal AFTER INSERT ON geo_resolution_changes FOR EACH ROW EXECUTE FUNCTION signal_listing_geography();

CREATE TRIGGER listing_geography_provider_signal AFTER UPDATE ON geo_resolution_provider_controls FOR EACH ROW EXECUTE FUNCTION signal_listing_geography();

CREATE TRIGGER listing_geography_listing_signal AFTER INSERT ON product_listing_events FOR EACH ROW WHEN (NEW.event_group='DOMAIN') EXECUTE FUNCTION signal_listing_geography();

REVOKE ALL ON SCHEMA dms_heartbeat FROM PUBLIC;

GRANT USAGE, CREATE ON SCHEMA dms_heartbeat TO aura_replication;

-- Keyset order for the cross-source admin directory, including retained withdrawals.
CREATE INDEX product_listings_admin_updated_id_idx ON product_listings (updated DESC, product_listing_id DESC);

CREATE INDEX product_listings_admin_created_id_idx ON product_listings (created DESC, product_listing_id DESC);

CREATE INDEX product_listings_admin_source_updated_id_idx ON product_listings (listing_source_id, updated DESC, product_listing_id DESC);

CREATE INDEX search_filters_periodic_match_eligible_idx ON search_filters (user_search_filter_id)
    WHERE state = 'ACTIVE';

CREATE UNIQUE INDEX search_filter_matching_one_running ON search_filter_matching_runs (status) WHERE status = 'RUNNING';

CREATE INDEX search_filter_matching_run_user ON search_filter_matching_run_filters(window_end, user_id, terminal);

CREATE INDEX search_filter_match_pending_selection ON search_filter_matches(notification_recovery_after, created, origin_event_id, user_search_filter_id, product_listing_id)
    WHERE matching_run_end IS NOT NULL AND NOT notification_selection_complete;

-- Immediate finalization probes one user/run after every selection. Keep this
-- lookup independent of the size of other users' pending recovery backlog.
CREATE INDEX search_filter_match_pending_group ON search_filter_matches(matching_run_end, user_id)
    WHERE matching_run_end IS NOT NULL AND NOT notification_selection_complete;

CREATE INDEX saved_search_digest_unassigned ON saved_search_digest_members(user_id, matching_run_end, created)
    WHERE digest_id IS NULL;

CREATE INDEX saved_search_digest_membership ON saved_search_digest_members(digest_id, created, origin_event_id, user_search_filter_id, product_listing_id);

CREATE TRIGGER fence_saved_search_individual_email BEFORE INSERT ON notification_deliveries
    FOR EACH ROW EXECUTE FUNCTION fence_saved_search_individual_email();

CREATE VIEW saved_search_digest_status AS
SELECT g.window_end, g.user_id, r.status AS run_status, r.recovery_deadline,
       g.finalized_at, d.digest_id, delivery.notification_delivery_id,
       delivery.status AS delivery_status, delivery.attempt_count, delivery.last_error_code,
       EXTRACT(EPOCH FROM now() - r.created) AS age_seconds,
       (SELECT count(*) FROM search_filter_matching_run_filters f WHERE f.window_end = g.window_end AND f.user_id = g.user_id AND NOT f.terminal) AS filters_pending,
       (SELECT count(*) FROM search_filter_matches m WHERE m.matching_run_end = g.window_end AND m.user_id = g.user_id AND NOT m.notification_selection_complete) AS selections_pending,
       (SELECT count(*) FROM saved_search_digest_members m WHERE m.digest_id = d.digest_id) AS eligible_matches,
       (SELECT count(DISTINCT product_listing_id) FROM saved_search_digest_members m WHERE m.digest_id = d.digest_id) AS deduplicated_listings
FROM saved_search_digest_groups g JOIN search_filter_matching_runs r USING(window_end)
LEFT JOIN saved_search_digests d USING(window_end, user_id)
LEFT JOIN notification_deliveries delivery ON delivery.digest_id = d.digest_id;

CREATE INDEX shipping_profiles_source_idx ON shipping_profiles(listing_source_id, shipping_profile_id);

CREATE INDEX shipping_source_defaults_profile_idx ON shipping_source_defaults(shipping_profile_id, listing_source_id) WHERE shipping_profile_id IS NOT NULL;

CREATE INDEX shipping_profile_locations_reverse_idx ON shipping_profile_location_references(party_location_id, shipping_profile_id);

CREATE INDEX shipping_changes_source_idx ON shipping_changes(listing_source_id, recorded, subject_id, revision);

CREATE TRIGGER shipping_source_deletion BEFORE DELETE ON listing_sources FOR EACH ROW EXECUTE FUNCTION tombstone_shipping_source();

CREATE TRIGGER shipping_changes_immutable BEFORE UPDATE OR DELETE ON shipping_changes FOR EACH ROW EXECUTE FUNCTION reject_shipping_history_mutation();

CREATE TRIGGER shipping_history_immutable BEFORE UPDATE OR DELETE ON shipping_profile_history FOR EACH ROW EXECUTE FUNCTION reject_shipping_history_mutation();

CREATE TRIGGER shipping_evidence_immutable BEFORE UPDATE OR DELETE ON shipping_profile_evidence FOR EACH ROW EXECUTE FUNCTION reject_shipping_history_mutation();

CREATE TRIGGER shipping_receipts_immutable BEFORE UPDATE OR DELETE ON shipping_profile_creation_receipts FOR EACH ROW EXECUTE FUNCTION reject_shipping_history_mutation();

CREATE INDEX product_listings_delivery_profile_idx
    ON product_listings ((delivery_configuration #>> '{selection,profile_id}'))
    WHERE delivery_configuration #>> '{selection,kind}' = 'EXPLICIT';

CREATE INDEX product_listings_delivery_source_idx ON product_listings (listing_source_id)
    WHERE delivery_configuration IS NOT NULL;

CREATE TRIGGER shipping_assertion_verification BEFORE UPDATE ON shipping_profiles
    FOR EACH ROW EXECUTE FUNCTION shipping_assertion_verification();

CREATE TRIGGER listing_delivery_verification BEFORE INSERT OR UPDATE ON product_listings
    FOR EACH ROW EXECUTE FUNCTION listing_delivery_verification();

CREATE TRIGGER initialize_shipping_import_authority AFTER INSERT ON listing_sources FOR EACH ROW EXECUTE FUNCTION initialize_shipping_import_authority();

CREATE INDEX shipping_validity_changes_source_idx ON shipping_validity_changes(listing_source_id,recorded);

CREATE TRIGGER shipping_validity_changes_immutable BEFORE UPDATE OR DELETE ON shipping_validity_changes
    FOR EACH ROW EXECUTE FUNCTION reject_shipping_history_mutation();

CREATE TRIGGER shipping_import_source_deleted BEFORE DELETE ON listing_sources FOR EACH ROW EXECUTE FUNCTION shipping_import_source_fence();

CREATE TRIGGER shipping_import_source_rebound AFTER UPDATE OF operator_party_id ON listing_sources FOR EACH ROW WHEN(OLD.operator_party_id IS DISTINCT FROM NEW.operator_party_id) EXECUTE FUNCTION shipping_import_source_fence();

CREATE TRIGGER shipping_import_method_changed AFTER INSERT OR DELETE ON listing_source_ingestion_methods FOR EACH ROW EXECUTE FUNCTION shipping_import_source_fence();

CREATE TRIGGER shipping_import_shopify_rebound AFTER INSERT OR DELETE OR UPDATE ON listing_source_shopify_ingestion_configurations FOR EACH ROW EXECUTE FUNCTION shipping_import_source_fence();

CREATE TRIGGER shipping_import_woocommerce_rebound AFTER INSERT OR DELETE OR UPDATE ON listing_source_woocommerce_ingestion_configurations FOR EACH ROW EXECUTE FUNCTION shipping_import_source_fence();

CREATE TRIGGER shipping_import_grant_revoked BEFORE DELETE ON partnership_listing_source_grants FOR EACH ROW EXECUTE FUNCTION shipping_import_source_fence();

CREATE TRIGGER shipping_import_membership_revoked BEFORE DELETE ON partnership_members FOR EACH ROW EXECUTE FUNCTION shipping_import_member_fence();

CREATE TRIGGER shipping_import_user_changed BEFORE DELETE OR UPDATE OF suspended,role ON users FOR EACH ROW EXECUTE FUNCTION shipping_import_member_fence();

CREATE TRIGGER shipping_import_partnership_changed BEFORE UPDATE OF business_state,party_id ON partnerships FOR EACH ROW EXECUTE FUNCTION shipping_import_partnership_fence();

CREATE TRIGGER shipping_profile_manual_ownership AFTER UPDATE ON shipping_profiles FOR EACH ROW EXECUTE FUNCTION shipping_profile_manual_ownership();

CREATE TRIGGER shipping_default_manual_ownership AFTER INSERT OR UPDATE ON shipping_source_defaults FOR EACH ROW EXECUTE FUNCTION shipping_default_manual_ownership();

CREATE TRIGGER listing_delivery_manual_ownership AFTER UPDATE ON product_listings FOR EACH ROW EXECUTE FUNCTION listing_delivery_manual_ownership();

CREATE INDEX listing_shipping_dependencies_reverse_idx ON listing_shipping_dependencies(dependency_kind,dependency_id,product_listing_id);

CREATE INDEX listing_shipping_work_pending_idx ON listing_shipping_work(pending_since,work_id) WHERE pending;

CREATE TRIGGER listing_shipping_management_signal AFTER INSERT ON shipping_changes FOR EACH ROW EXECUTE FUNCTION signal_listing_shipping();

CREATE TRIGGER listing_shipping_validity_signal AFTER INSERT ON shipping_validity_changes FOR EACH ROW EXECUTE FUNCTION signal_listing_shipping();

CREATE TRIGGER listing_shipping_listing_signal AFTER INSERT ON product_listing_events FOR EACH ROW WHEN(NEW.event_group='DOMAIN') EXECUTE FUNCTION signal_listing_shipping();

CREATE TRIGGER listing_shipping_location_signal AFTER INSERT ON party_location_changes FOR EACH ROW EXECUTE FUNCTION signal_listing_shipping();

CREATE TRIGGER listing_shipping_resolution_signal AFTER INSERT ON geo_resolution_changes FOR EACH ROW EXECUTE FUNCTION signal_listing_shipping();

CREATE TRIGGER listing_shipping_rights_signal AFTER UPDATE ON geo_resolution_provider_controls FOR EACH ROW EXECUTE FUNCTION signal_listing_shipping();

CREATE INDEX product_listing_normalization_diagnostic_cases_raw_revision_idx
    ON product_listing_normalization_diagnostic_cases (product_listing_raw_revision_id);

CREATE INDEX product_listing_normalization_diagnostic_cases_source_idx
    ON product_listing_normalization_diagnostic_cases (listing_source_id, generation);

COMMENT ON COLUMN oauth_authorization_codes.resource IS
    'Exact Aura resource identifier approved for this code; NULL uses the configured issuer shared-resource default.';

INSERT INTO geo_resolution_provider_controls(profile,is_default)
VALUES ('AMAZON_LOCATION_PLACES_V2_DEFAULT_FRANKFURT_2026_10',true);
