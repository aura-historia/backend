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
