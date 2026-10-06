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
