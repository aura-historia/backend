-- Content notices, phase 1: notice intake (docs/content-notices.md) and transactional
-- email to addresses that do not belong to a user account, which delivers the notice
-- receipt (docs/events/flow.md, External email delivery).

---------------------------------------------------------------------------------------------
-- Content notices: requests from providers, rights holders and reporters of illegal content
-- to review a listing or a listing source.
--
-- A notice is evidence. It deliberately has no foreign key to listing_sources or
-- product_listings: it must neither block nor cascade with the deletion of its target, so the
-- target is kept as plain identifiers plus a snapshot taken at submission.
--
-- The table holds personal data (name, email, explanation, URLs). It is not captured by
-- CDC and has no projection.
---------------------------------------------------------------------------------------------

-- Free text is stored trimmed, as the domain types produce it.
--
-- Additional URLs are bounded, absolute http(s) URLs without repetition.
CREATE FUNCTION aura_content_notice_additional_urls_valid(urls text[])
RETURNS boolean
LANGUAGE sql
IMMUTABLE
STRICT
PARALLEL SAFE
AS $$
    SELECT coalesce(
            bool_and(char_length(url) BETWEEN 1 AND 2048 AND url ~ '^https?://'),
            true
        )
        AND count(*) = count(DISTINCT url)
    FROM unnest(urls) AS url
$$;

CREATE TABLE content_notices (
    content_notice_id uuid PRIMARY KEY,
    target_type text NOT NULL,
    listing_source_id uuid NOT NULL,
    product_listing_id uuid,
    listing_source_name text NOT NULL,
    listing_source_url text,
    listing_source_partner boolean NOT NULL,
    product_listing_title text,
    product_listing_url text,
    submitter_role text NOT NULL,
    reason_category text NOT NULL,
    explanation text,
    additional_urls text[] NOT NULL DEFAULT '{}',
    submitter_name text NOT NULL,
    submitter_email text NOT NULL,
    -- Groups submissions per address for the submission limits.
    submitter_email_key text GENERATED ALWAYS AS (lower(submitter_email)) STORED NOT NULL,
    submitter_language text NOT NULL,
    good_faith_statement_version text NOT NULL,
    -- SHA-256 of the submission; the submission use case defines what it covers.
    submission_fingerprint bytea NOT NULL,
    business_state text NOT NULL,
    version bigint NOT NULL DEFAULT 1,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT content_notices_target_type_check
        CHECK (target_type IN ('LISTING_SOURCE', 'PRODUCT_LISTING')),
    CONSTRAINT content_notices_target_check CHECK (
        (target_type = 'LISTING_SOURCE'
            AND product_listing_id IS NULL
            AND product_listing_title IS NULL
            AND product_listing_url IS NULL)
        OR (target_type = 'PRODUCT_LISTING'
            AND product_listing_id IS NOT NULL
            AND product_listing_url IS NOT NULL)
    ),
    CONSTRAINT content_notices_submitter_role_check
        CHECK (submitter_role IN ('PROVIDER', 'RIGHTS_HOLDER', 'ILLEGAL_CONTENT_REPORTER')),
    CONSTRAINT content_notices_reason_category_check CHECK (
        reason_category IN (
            'PROVIDER_OPT_OUT',
            'RIGHTS_INFRINGEMENT',
            'ILLEGAL_CONTENT',
            'INCORRECT_INFORMATION',
            'OTHER'
        )
    ),
    CONSTRAINT content_notices_provider_opt_out_role_check
        CHECK (reason_category <> 'PROVIDER_OPT_OUT' OR submitter_role = 'PROVIDER'),
    CONSTRAINT content_notices_explanation_required_check
        CHECK (explanation IS NOT NULL OR reason_category = 'PROVIDER_OPT_OUT'),
    CONSTRAINT content_notices_submitter_language_check CHECK (
        submitter_language IN (
            'de', 'en', 'fr', 'es', 'it', 'zh', 'pt', 'pl', 'tr', 'nl',
            'cs', 'ja', 'ru', 'ar', 'ko', 'el', 'he', 'th', 'hi'
        )
    ),
    CONSTRAINT content_notices_good_faith_statement_version_check
        CHECK (good_faith_statement_version IN ('GOOD_FAITH_V1')),
    CONSTRAINT content_notices_business_state_check CHECK (business_state IN ('SUBMITTED')),
    CONSTRAINT content_notices_listing_source_name_check CHECK (
        length(trim(listing_source_name)) > 0 AND octet_length(listing_source_name) <= 255
    ),
    CONSTRAINT content_notices_listing_source_url_check
        CHECK (listing_source_url IS NULL OR char_length(listing_source_url) > 0),
    CONSTRAINT content_notices_product_listing_title_check CHECK (
        product_listing_title IS NULL
        OR (char_length(product_listing_title) BETWEEN 1 AND 500
            AND product_listing_title = btrim(product_listing_title, E' \t\n\r')
            AND product_listing_title !~ '[\u0001-\u001F\u007F-\u009F]')
    ),
    CONSTRAINT content_notices_product_listing_url_check
        CHECK (product_listing_url IS NULL OR char_length(product_listing_url) > 0),
    CONSTRAINT content_notices_explanation_check CHECK (
        explanation IS NULL
        OR (char_length(explanation) BETWEEN 1 AND 5000
            AND explanation = btrim(explanation, E' \t\n\r')
            AND explanation !~ '[\u0001-\u0008\u000B\u000C\u000E-\u001F\u007F-\u009F]')
    ),
    CONSTRAINT content_notices_additional_urls_count_check
        CHECK (cardinality(additional_urls) <= 10),
    CONSTRAINT content_notices_additional_urls_check
        CHECK (aura_content_notice_additional_urls_valid(additional_urls)),
    CONSTRAINT content_notices_submitter_name_check CHECK (
        char_length(submitter_name) BETWEEN 1 AND 200
        AND submitter_name = btrim(submitter_name, E' \t\n\r')
        AND submitter_name !~ '[\u0001-\u001F\u007F-\u009F]'
    ),
    -- ASCII, with an international domain in its punycode form, so lower() agrees with the
    -- submission fingerprint's lowercasing and the email provider accepts the address.
    CONSTRAINT content_notices_submitter_email_check CHECK (
        octet_length(submitter_email) BETWEEN 1 AND 320
        AND octet_length(submitter_email) = char_length(submitter_email)
        AND submitter_email !~ '[[:space:][:cntrl:]<>]'
    ),
    CONSTRAINT content_notices_submission_fingerprint_check
        CHECK (octet_length(submission_fingerprint) = 32),
    CONSTRAINT content_notices_version_positive CHECK (version >= 1)
);

-- Admin list, newest first.
CREATE INDEX content_notices_created_idx
    ON content_notices (created DESC, content_notice_id DESC);

-- Submission limits per address within a time window.
CREATE INDEX content_notices_submitter_email_key_created_idx
    ON content_notices (submitter_email_key, created);

-- Admin filter by listing source; covers notices about the source and about its listings.
CREATE INDEX content_notices_listing_source_created_idx
    ON content_notices (listing_source_id, created DESC, content_notice_id DESC);

-- Admin filter by listing.
CREATE INDEX content_notices_product_listing_created_idx
    ON content_notices (product_listing_id, created DESC, content_notice_id DESC)
    WHERE product_listing_id IS NOT NULL;

---------------------------------------------------------------------------------------------
-- Scope content-notices:read. Registered OAuth clients and the credentials issued to them or
-- created by users may carry it. Metadata-document clients and their connections stay
-- limited to the ordinary-user scope vocabulary (docs/oauth.md), so their constraints are
-- unchanged.
---------------------------------------------------------------------------------------------

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

---------------------------------------------------------------------------------------------
-- External email. A producer inserts the message and its notification_deliveries row in its
-- own business transaction; the existing notification delivery pipeline sends it.
--
-- The table holds personal data (recipient address and template variables). It is not
-- captured by CDC; only the delivery primary key enters Kinesis. Each origin owns the
-- retention of its messages and erases them by origin; deliveries cascade.
---------------------------------------------------------------------------------------------

CREATE TABLE external_email_messages (
    external_email_message_id uuid PRIMARY KEY,
    recipient_email text NOT NULL,
    language text NOT NULL,
    template text NOT NULL,
    payload_version integer NOT NULL,
    payload jsonb NOT NULL,
    origin_type text NOT NULL,
    origin_id uuid NOT NULL,
    -- A message is immutable once enqueued; its delivery carries the progress.
    created timestamptz NOT NULL DEFAULT now(),
    -- An ASCII address (an international domain in its punycode form), as the email
    -- provider requires: every character is one byte.
    CONSTRAINT external_email_messages_recipient_email_check CHECK (
        octet_length(recipient_email) BETWEEN 3 AND 320
        AND octet_length(recipient_email) = char_length(recipient_email)
        AND recipient_email !~ '[[:space:][:cntrl:]<>]'
        AND position('@' IN recipient_email) > 1
    ),
    CONSTRAINT external_email_messages_language_check CHECK (
        language IN (
            'de', 'en', 'fr', 'es', 'it', 'zh', 'pt', 'pl', 'tr', 'nl',
            'cs', 'ja', 'ru', 'ar', 'ko', 'el', 'he', 'th', 'hi'
        )
    ),
    -- One value per supported template kind.
    CONSTRAINT external_email_messages_template_check
        CHECK (template IN ('CONTENT_NOTICE_RECEIPT')),
    CONSTRAINT external_email_messages_payload_version_check CHECK (payload_version >= 1),
    CONSTRAINT external_email_messages_payload_check CHECK (jsonb_typeof(payload) = 'object'),
    CONSTRAINT external_email_messages_origin_type_check
        CHECK (origin_type ~ '^[A-Z][A-Z0-9_]{0,63}$'),
    -- One origin cannot enqueue the same email twice.
    CONSTRAINT external_email_messages_origin_template_unique
        UNIQUE (origin_type, origin_id, template)
);

-- notification_deliveries is captured by CDC, so this release needs approved DMS
-- maintenance. Adding the nullable column is metadata-only, but this statement builds the
-- unique index and checks the new constraints against existing rows under its exclusive
-- lock, which blocks delivery writes until it ends; the migration runner's lock and
-- statement timeouts bound the wait. Existing rows have no external email message, so every
-- check holds for them.
ALTER TABLE notification_deliveries
    ADD COLUMN external_email_message_id uuid,
    ADD CONSTRAINT notification_deliveries_external_email_message_id_key
        UNIQUE (external_email_message_id),
    ADD CONSTRAINT notification_deliveries_external_email_message_id_fkey
        FOREIGN KEY (external_email_message_id)
        REFERENCES external_email_messages ON DELETE CASCADE,
    DROP CONSTRAINT notification_delivery_source_check,
    ADD CONSTRAINT notification_delivery_source_check
        CHECK (num_nonnulls(notification_id, digest_id, external_email_message_id) = 1),
    ADD CONSTRAINT notification_deliveries_external_email_target_check CHECK (
        external_email_message_id IS NULL
        OR (channel = 'EMAIL' AND target_key = 'RECIPIENT')
    );
