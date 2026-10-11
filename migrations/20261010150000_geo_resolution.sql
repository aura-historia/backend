-- Private authoritative inputs/results. Only geo_resolution_wakeups is selected for CDC.
CREATE TABLE geo_resolution_provider_controls (
    profile text PRIMARY KEY,
    is_default boolean NOT NULL DEFAULT false,
    enabled boolean NOT NULL DEFAULT false,
    storage_permitted boolean NOT NULL DEFAULT false,
    daily_limit integer NOT NULL DEFAULT 100 CHECK (daily_limit BETWEEN 1 AND 100000),
    scope_daily_limit integer NOT NULL DEFAULT 20 CHECK (scope_daily_limit BETWEEN 1 AND 100000),
    concurrency_limit integer NOT NULL DEFAULT 2 CHECK (concurrency_limit BETWEEN 1 AND 8),
    scope_concurrency_limit integer NOT NULL DEFAULT 1 CHECK (scope_concurrency_limit BETWEEN 1 AND 8),
    budget_day date NOT NULL DEFAULT (now() AT TIME ZONE 'UTC')::date,
    calls integer NOT NULL DEFAULT 0 CHECK (calls >= 0),
    consecutive_failures integer NOT NULL DEFAULT 0 CHECK (consecutive_failures >= 0),
    open_until timestamptz
);
CREATE UNIQUE INDEX geo_resolution_default_profile_idx ON geo_resolution_provider_controls(is_default) WHERE is_default;
INSERT INTO geo_resolution_provider_controls(profile,is_default)
VALUES ('AMAZON_LOCATION_PLACES_V2_DEFAULT_FRANKFURT_2026_10',true);

CREATE TABLE geo_resolution_scope_budgets (
    profile text NOT NULL REFERENCES geo_resolution_provider_controls(profile),
    scope_key text NOT NULL CHECK (scope_key ~ '^[0-9a-f]{64}$'),
    budget_day date NOT NULL,
    calls integer NOT NULL CHECK (calls >= 0),
    PRIMARY KEY(profile,scope_key)
);
CREATE TABLE geo_resolutions (
    resolution_id uuid PRIMARY KEY,
    fingerprint text NOT NULL UNIQUE CHECK (fingerprint ~ '^[0-9a-f]{64}$'),
    profile text NOT NULL REFERENCES geo_resolution_provider_controls(profile),
    policy_version text NOT NULL CHECK (policy_version = 'PRECISION_FIRST_V1'),
    scope_key text NOT NULL CHECK (scope_key ~ '^[0-9a-f]{64}$'),
    input jsonb NOT NULL,
    generation bigint NOT NULL DEFAULT 1 CHECK (generation > 0),
    status text NOT NULL DEFAULT 'PENDING' CHECK (status IN ('PENDING','ACCEPTED','AMBIGUOUS','NO_MATCH','FAILED','BLOCKED')),
    attempts integer NOT NULL DEFAULT 0 CHECK (attempts BETWEEN 0 AND 5),
    lease_token uuid,
    lease_until timestamptz,
    next_attempt_at timestamptz NOT NULL DEFAULT now(),
    evidence jsonb,
    valid_until timestamptz,
    last_failure text,
    created timestamptz NOT NULL DEFAULT now(),
    pending_since timestamptz NOT NULL DEFAULT now(),
    CHECK ((lease_token IS NULL) = (lease_until IS NULL)),
    CHECK ((evidence IS NULL) = (valid_until IS NULL)),
    CHECK ((status IN ('ACCEPTED','AMBIGUOUS','NO_MATCH')) = (evidence IS NOT NULL)),
    CHECK (status NOT IN ('FAILED','BLOCKED') OR evidence IS NULL)
);
CREATE INDEX geo_resolution_pending_idx ON geo_resolutions(next_attempt_at,created) WHERE status='PENDING';
CREATE INDEX geo_resolution_expiry_idx ON geo_resolutions(valid_until) WHERE evidence IS NOT NULL;

-- Bound only by owning services under their authorization/aggregate lock. Removing or
-- changing a binding immediately makes the old geography ineligible for that subject.
CREATE TABLE geo_resolution_bindings (
    subject_kind text NOT NULL CHECK (subject_kind IN ('PARTY_LOCATION','USER_LOCATION')),
    subject_id uuid NOT NULL,
    resolution_id uuid NOT NULL REFERENCES geo_resolutions(resolution_id),
    input_revision bigint NOT NULL CHECK (input_revision > 0),
    guard bigint NOT NULL CHECK (guard > 0),
    selected_candidate integer CHECK (selected_candidate BETWEEN 0 AND 10),
    selected_generation bigint CHECK (selected_generation > 0),
    reviewed_actor text,
    PRIMARY KEY(subject_kind,subject_id),
    CHECK ((selected_candidate IS NULL) = (selected_generation IS NULL)),
    CHECK ((selected_candidate IS NULL) = (reviewed_actor IS NULL))
);
CREATE INDEX geo_resolution_binding_resolution_idx ON geo_resolution_bindings(resolution_id);

-- Compact immutable wake-ups: generations fence old retries/replays.
CREATE TABLE geo_resolution_wakeups (
    resolution_id uuid NOT NULL REFERENCES geo_resolutions(resolution_id),
    generation bigint NOT NULL CHECK (generation > 0),
    PRIMARY KEY(resolution_id,generation)
);
-- Durable enrichment/invalidation signal, including binding review and expiry. It
-- contains no provider evidence and is reserved for dependent-geography consumers.
CREATE TABLE geo_resolution_changes (
    change_id uuid PRIMARY KEY,
    resolution_id uuid NOT NULL REFERENCES geo_resolutions(resolution_id),
    generation bigint NOT NULL CHECK (generation > 0),
    change_kind text NOT NULL CHECK (change_kind IN ('RESOLVED','INVALIDATED','REVIEWED','REQUEUED')),
    subject_kind text,
    subject_id uuid,
    occurred_at timestamptz NOT NULL DEFAULT now(),
    CHECK ((subject_kind IS NULL) = (subject_id IS NULL))
);
