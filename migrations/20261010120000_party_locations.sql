-- Party associations with independently identified sites; no global address identity/default.
CREATE TABLE party_locations (
    party_location_id uuid PRIMARY KEY,
    party_id uuid NOT NULL REFERENCES parties(party_id) ON DELETE RESTRICT,
    label text NOT NULL CHECK (octet_length(label) BETWEEN 1 AND 255 AND length(trim(label)) > 0),
    roles text[] NOT NULL DEFAULT '{}' CHECK (roles <@ ARRAY['BUSINESS_PREMISES','WAREHOUSE','REGISTERED_ADDRESS','CORRESPONDENCE']::text[] AND cardinality(roles) <= 4),
    geography jsonb,
    position jsonb,
    disclosure text NOT NULL CHECK (disclosure IN ('PRIVATE','COARSE_PUBLIC','EXACT_PUBLIC')),
    lifecycle text NOT NULL CHECK (lifecycle IN ('ACTIVE','RETIRED')),
    revision bigint NOT NULL DEFAULT 1 CHECK (revision > 0),
    input_revision bigint NOT NULL DEFAULT 1 CHECK (input_revision > 0 AND input_revision <= revision),
    evidence jsonb,
    created_actor text NOT NULL,
    updated_actor text NOT NULL,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX party_locations_party_cursor_idx ON party_locations(party_id, party_location_id);

-- All access/grant changes serialize on the owner Party row. Listing scopes confer no grant.
CREATE TABLE party_location_management_grants (
    party_id uuid NOT NULL REFERENCES parties(party_id) ON DELETE RESTRICT,
    user_id uuid NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    granted_actor text NOT NULL,
    PRIMARY KEY (party_id, user_id)
);

-- Private completed-command receipts. No TTL, CDC selection or normalized-address identity.
CREATE TABLE party_location_create_receipts (
    party_id uuid NOT NULL REFERENCES parties(party_id) ON DELETE RESTRICT,
    actor text NOT NULL,
    idempotency_key text NOT NULL CHECK (octet_length(idempotency_key) BETWEEN 1 AND 128),
    command jsonb NOT NULL,
    result jsonb NOT NULL,
    PRIMARY KEY (party_id, actor, idempotency_key)
);

-- Minimal immutable change signal for later dependent-geography invalidation. No raw evidence.
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
