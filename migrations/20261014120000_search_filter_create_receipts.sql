-- Private application outcomes, committed with creation. Intentionally excluded from CDC.
-- Filter deletion erases the snapshot but preserves command identity until account erasure.
CREATE TABLE search_filter_create_receipts (
    user_id uuid NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    actor text NOT NULL CHECK (octet_length(actor) BETWEEN 1 AND 512),
    idempotency_key text NOT NULL CHECK (
        octet_length(idempotency_key) BETWEEN 1 AND 128
        AND idempotency_key ~ '^[A-Za-z0-9_.:-]+$'
    ),
    fingerprint_version integer NOT NULL CHECK (fingerprint_version = 1),
    fingerprint bytea NOT NULL CHECK (octet_length(fingerprint) = 32),
    user_search_filter_id uuid NOT NULL,
    outcome_version integer NOT NULL CHECK (outcome_version = 1),
    outcome jsonb CHECK (outcome IS NULL OR octet_length(outcome::text) <= 1048576),
    PRIMARY KEY (user_id, actor, idempotency_key)
);

CREATE INDEX search_filter_create_receipts_filter_idx
    ON search_filter_create_receipts(user_search_filter_id) WHERE outcome IS NOT NULL;
