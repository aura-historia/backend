-- Service-owned frozen inputs/checkpoints; no raw credentials, tokens or provider payloads.
CREATE TABLE external_command_operations (
    actor text NOT NULL CHECK(octet_length(actor) BETWEEN 1 AND 512),
    operation text NOT NULL CHECK(operation ~ '^[A-Z][A-Z0-9_]{1,95}$'),
    target text NOT NULL CHECK(octet_length(target)<=512),
    idempotency_key text NOT NULL CHECK(octet_length(idempotency_key) BETWEEN 1 AND 128 AND idempotency_key ~ '^[A-Za-z0-9_.:-]+$'),
    private_owner uuid REFERENCES users(user_id) ON DELETE CASCADE,
    fingerprint_version integer NOT NULL DEFAULT 1 CHECK(fingerprint_version=1),
    fingerprint bytea NOT NULL CHECK(octet_length(fingerprint)=32),
    state_version integer NOT NULL DEFAULT 1 CHECK(state_version=1),
    state jsonb CHECK(state IS NULL OR octet_length(state::text)<=1048576),
    admitted_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    lease_until timestamptz NOT NULL DEFAULT (clock_timestamp()+interval '60 seconds'),
    attempt bigint NOT NULL DEFAULT 1 CHECK(attempt>=1),
    completed boolean NOT NULL DEFAULT false,
    exclusive_scope text NOT NULL CHECK(octet_length(exclusive_scope)<=512),
    evidence_expires_at timestamptz NOT NULL DEFAULT (now()+interval '30 days'),
    PRIMARY KEY(actor,operation,target,idempotency_key)
);
CREATE UNIQUE INDEX external_command_operations_unresolved_scope ON external_command_operations(exclusive_scope) WHERE NOT completed;
CREATE INDEX external_command_operations_expiry ON external_command_operations(evidence_expires_at) WHERE completed AND state IS NOT NULL;
