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
