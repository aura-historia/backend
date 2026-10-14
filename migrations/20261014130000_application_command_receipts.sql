-- Application outcomes, never HTTP responses. No CDC selection or business-history projection.
CREATE TABLE application_command_receipts (
    actor text NOT NULL CHECK (octet_length(actor) BETWEEN 1 AND 512),
    operation text NOT NULL CHECK (operation ~ '^[A-Z][A-Z0-9_]{1,95}$'),
    target text NOT NULL CHECK (octet_length(target) <= 512),
    idempotency_key text NOT NULL CHECK (octet_length(idempotency_key) BETWEEN 1 AND 128 AND idempotency_key ~ '^[A-Za-z0-9_.:-]+$'),
    private_owner uuid REFERENCES users(user_id) ON DELETE CASCADE,
    fingerprint_version integer NOT NULL CHECK (fingerprint_version=1),
    fingerprint bytea CHECK (octet_length(fingerprint)=32),
    outcome_version integer NOT NULL CHECK (outcome_version=1),
    outcome jsonb CHECK (outcome IS NULL OR octet_length(outcome::text)<=1048576),
    outcome_expires_at timestamptz NOT NULL DEFAULT (now()+interval '30 days'),
    CHECK (fingerprint IS NOT NULL OR outcome IS NULL),
    PRIMARY KEY(actor,operation,target,idempotency_key)
);
CREATE INDEX application_command_receipts_expired_idx ON application_command_receipts(outcome_expires_at) WHERE outcome IS NOT NULL;
CREATE INDEX application_command_receipts_private_owner_idx ON application_command_receipts(private_owner) WHERE private_owner IS NOT NULL;
