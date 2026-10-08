-- Consolidates migrations added after deployment of 4b15817e1dae7700ae7dc48c3ab1eb4a903e4001.
-- Preserve the original statement order; previously deployed migrations remain unchanged.

-- Listing source provider write scope
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

-- Search filter matches drop enhanced match reason
ALTER TABLE search_filter_matches
    DROP COLUMN enhanced_match_reason;

-- User marketing email consent
ALTER TABLE users
  ADD COLUMN marketing_email_consent boolean NOT NULL DEFAULT false;

-- Marketing email consent sync intents
-- The decision, its independent revision and the immutable synchronization target
-- are written in one service-owned transaction. Existing users retain consent=false.
ALTER TABLE users
    ADD COLUMN marketing_email_consent_revision bigint NOT NULL DEFAULT 0,
    ADD COLUMN marketing_email_consent_changed_at timestamptz,
    ADD CONSTRAINT users_marketing_email_consent_revision_nonnegative
        CHECK (marketing_email_consent_revision >= 0);

-- User ID is deliberately a snapshot, not a cascading FK. A revoke must survive
-- deletion without ever becoming an anonymous grant.
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
    CONSTRAINT marketing_email_consent_sync_intents_subject_check CHECK (
        (subject_type = 'USER' AND user_id IS NOT NULL AND consent_revision > 0
            AND source IN ('COGNITO_SIGNUP', 'AURA_DOUBLE_OPT_IN', 'USER_WITHDRAWAL', 'USER_DELETION'))
        OR (subject_type = 'EMAIL_ONLY' AND user_id IS NULL AND consent_revision IS NULL
            AND source IN ('AURA_DOUBLE_OPT_IN', 'EMAIL_ONLY_WITHDRAWAL'))
    ),
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
    )
);
CREATE INDEX marketing_email_consent_sync_intents_claim_idx
    ON marketing_email_consent_sync_intents (intent_sequence)
    WHERE status IN ('PENDING', 'IN_PROGRESS');
CREATE INDEX marketing_email_consent_sync_intents_recipient_idx
    ON marketing_email_consent_sync_intents (recipient_key, intent_sequence DESC);
CREATE INDEX marketing_email_consent_sync_intents_user_idx
    ON marketing_email_consent_sync_intents (user_id, intent_sequence DESC) WHERE user_id IS NOT NULL;

-- Marketing consent provider race repair
-- A repair is operational compensation, never new user consent evidence.
ALTER TABLE marketing_email_consent_sync_intents
    DROP CONSTRAINT marketing_email_consent_sync_intents_subject_check,
    ADD CONSTRAINT marketing_email_consent_sync_intents_subject_check CHECK (
        (subject_type = 'USER' AND user_id IS NOT NULL AND consent_revision > 0
            AND source IN ('COGNITO_SIGNUP', 'AURA_DOUBLE_OPT_IN', 'USER_WITHDRAWAL', 'USER_DELETION', 'PROVIDER_RACE_REPAIR'))
        OR (subject_type = 'EMAIL_ONLY' AND user_id IS NULL AND consent_revision IS NULL
            AND source IN ('AURA_DOUBLE_OPT_IN', 'EMAIL_ONLY_WITHDRAWAL', 'PROVIDER_RACE_REPAIR'))
    ),
    ADD CONSTRAINT marketing_email_consent_sync_intents_repair_revoke_check CHECK (
        source <> 'PROVIDER_RACE_REPAIR' OR NOT desired
    );

-- Newsletter subscription confirmations
-- One operational proof row per DOI issuance. Raw URL tokens and rendered email
-- content are never persisted; the token digest is sufficient for lookup.
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

CREATE INDEX newsletter_subscription_confirmations_recipient_created_idx
    ON newsletter_subscription_confirmations (recipient_key, email, created_at DESC);
CREATE INDEX newsletter_subscription_confirmations_expiry_idx
    ON newsletter_subscription_confirmations (expires_at, confirmation_id)
    WHERE confirmed_at IS NULL;
CREATE INDEX newsletter_subscription_confirmations_bound_user_idx
    ON newsletter_subscription_confirmations (bound_user_id)
    WHERE bound_user_id IS NOT NULL AND confirmed_at IS NULL;

-- Loops webhook preference events
-- Operational deduplication evidence only: never retain the signed request or
-- complete Loops payload. C15 owns bounded cleanup at expires_at.
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
CREATE INDEX loops_webhook_receipts_expiry_idx
    ON loops_webhook_receipts (expires_at, delivery_id);

-- The latest provider decision/contact binding survives receipt cleanup. The key
-- is the exact mailbox string; this table cannot rebind a User's account email.
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

-- Consent workflow cleanup indexes
-- Retention scans use completion times. Leave active/blocked/failed work outside
-- these indexes and outside automatic cleanup.
CREATE INDEX newsletter_subscription_confirmations_confirmed_cleanup_idx
    ON newsletter_subscription_confirmations (confirmed_at, confirmation_id)
    WHERE confirmed_at IS NOT NULL;

CREATE INDEX marketing_email_consent_sync_intents_completed_cleanup_idx
    ON marketing_email_consent_sync_intents (completed_at, intent_id)
    WHERE status IN ('APPLIED', 'SUPERSEDED') AND completed_at IS NOT NULL;

-- Older receipts may carry the previous 35-day expires_at. Their processed_at
-- still controls the minimum 120-day retention window.
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

-- Loops positive delivery tombstones
-- A positive webhook must remain spent after its detailed receipt expires. Keep
-- only the opaque delivery ID and exact-body digest; no mailbox or provider state.
CREATE TABLE loops_webhook_positive_delivery_tombstones (
    delivery_id text PRIMARY KEY,
    raw_body_sha256 bytea NOT NULL,
    CONSTRAINT loops_positive_delivery_id_check
        CHECK (octet_length(delivery_id) BETWEEN 1 AND 256 AND delivery_id !~ '[[:cntrl:]]'),
    CONSTRAINT loops_positive_delivery_digest_check
        CHECK (octet_length(raw_body_sha256) = 32)
);

-- Block old writers while copying existing positive receipts and installing the
-- transactional marker. This also covers writes from the old application during
-- the deployment interval before all webhook workers use the new lookup.
LOCK TABLE loops_webhook_receipts IN SHARE ROW EXCLUSIVE MODE;
INSERT INTO loops_webhook_positive_delivery_tombstones (delivery_id, raw_body_sha256)
SELECT delivery_id, raw_body_sha256 FROM loops_webhook_receipts
WHERE provider_event_name = 'email.resubscribed';

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

CREATE TRIGGER record_loops_positive_delivery
AFTER INSERT ON loops_webhook_receipts
FOR EACH ROW EXECUTE FUNCTION record_loops_positive_delivery();
