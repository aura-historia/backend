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
