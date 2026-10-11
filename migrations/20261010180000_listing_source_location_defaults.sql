-- One explicitly selected operator-owned location per source; no copied geography.
ALTER TABLE listing_sources ADD CONSTRAINT listing_sources_operator_identity UNIQUE (listing_source_id, operator_party_id);
ALTER TABLE party_locations ADD CONSTRAINT party_locations_owner_identity UNIQUE (party_location_id, party_id);
CREATE TABLE listing_source_location_defaults (
    listing_source_id uuid PRIMARY KEY,
    party_id uuid NOT NULL,
    party_location_id uuid,
    business_evidence jsonb,
    inventory_evidence jsonb,
    revision bigint NOT NULL CHECK (revision >= 2),
    updated_actor text NOT NULL,
    updated timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (listing_source_id, party_id) REFERENCES listing_sources(listing_source_id, operator_party_id) ON DELETE CASCADE,
    FOREIGN KEY (party_location_id, party_id) REFERENCES party_locations(party_location_id, party_id) ON DELETE RESTRICT,
    CHECK (party_location_id IS NOT NULL OR (business_evidence IS NULL AND inventory_evidence IS NULL)),
    CHECK (business_evidence IS NULL OR (jsonb_typeof(business_evidence) = 'object' AND business_evidence ?& ARRAY['reference','scope'] AND jsonb_typeof(business_evidence->'scope') = 'string' AND jsonb_typeof(business_evidence->'reference') = 'string' AND business_evidence->>'scope' = 'ASSOCIATED_BUSINESS' AND business_evidence - 'reference' - 'scope' = '{}'::jsonb)),
    CHECK (inventory_evidence IS NULL OR (jsonb_typeof(inventory_evidence) = 'object' AND inventory_evidence ?& ARRAY['reference','scope'] AND jsonb_typeof(inventory_evidence->'scope') = 'string' AND jsonb_typeof(inventory_evidence->'reference') = 'string' AND inventory_evidence->>'scope' = 'ADVERTISED_INVENTORY' AND inventory_evidence - 'reference' - 'scope' = '{}'::jsonb))
);
CREATE INDEX listing_source_location_defaults_location_idx ON listing_source_location_defaults(party_location_id) WHERE party_location_id IS NOT NULL;

-- Retained immutable invalidation basis, independent of source deletion. Never evidence/addresses.
CREATE TABLE listing_source_location_default_changes (
    listing_source_id uuid NOT NULL,
    revision bigint NOT NULL CHECK (revision >= 2),
    party_id uuid NOT NULL,
    party_location_id uuid,
    location_revision bigint CHECK (location_revision > 0),
    business_association boolean NOT NULL,
    advertised_inventory boolean NOT NULL,
    actor text NOT NULL,
    occurred_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (listing_source_id, revision),
    CHECK ((party_location_id IS NULL) = (location_revision IS NULL)),
    CHECK (party_location_id IS NOT NULL OR (NOT business_association AND NOT advertised_inventory))
);
