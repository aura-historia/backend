-- Retained replay corpus: every raw revision whose worker normalization ever produced an
-- unresolved or failed outcome. One row per versioned entry point and exact input identity;
-- retries and later evaluations update the recorded outcome/provenance without duplicating it.
CREATE TABLE product_listing_normalization_diagnostic_cases (
    case_id uuid PRIMARY KEY,
    generation bigint GENERATED ALWAYS AS IDENTITY UNIQUE,
    entry_point text NOT NULL,
    input_sha256 bytea NOT NULL,
    listing_source_id uuid NOT NULL,
    -- The immutable raw revision holds the complete input. Deleting the revision deletes the
    -- case, so every retained reference remains replayable.
    product_listing_raw_revision_id uuid NOT NULL
        REFERENCES product_listing_raw_revisions(product_listing_raw_revision_id) ON DELETE CASCADE,
    outcome text NOT NULL,
    issues jsonb NOT NULL,
    diagnostics_version smallint NOT NULL,
    evaluator_version smallint NOT NULL,
    build text NOT NULL,
    occurrences bigint NOT NULL DEFAULT 1,
    first_recorded_at timestamptz NOT NULL DEFAULT now(),
    last_recorded_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT product_listing_normalization_diagnostic_cases_identity_unique
        UNIQUE (entry_point, input_sha256),
    CONSTRAINT product_listing_normalization_diagnostic_cases_entry_point_check
        CHECK (entry_point IN ('RAW_VALUES_NORMALIZATION_V1')),
    CONSTRAINT product_listing_normalization_diagnostic_cases_input_sha256_length_check
        CHECK (octet_length(input_sha256) = 32),
    CONSTRAINT product_listing_normalization_diagnostic_cases_outcome_check
        CHECK (outcome IN (
            'APPLIED', 'NO_CHANGE', 'REJECTED', 'SYSTEM_FAILURE',
            'EVALUATED_RESOLVED', 'EVALUATED_INVALID'
        )),
    CONSTRAINT product_listing_normalization_diagnostic_cases_issues_array_check
        CHECK (jsonb_typeof(issues) = 'array'),
    CONSTRAINT product_listing_normalization_diagnostic_cases_versions_positive_check
        CHECK (diagnostics_version >= 1 AND evaluator_version >= 1),
    CONSTRAINT product_listing_normalization_diagnostic_cases_build_length_check
        CHECK (octet_length(build) BETWEEN 1 AND 64),
    CONSTRAINT product_listing_normalization_diagnostic_cases_occurrences_positive_check
        CHECK (occurrences >= 1)
);

CREATE INDEX product_listing_normalization_diagnostic_cases_raw_revision_idx
    ON product_listing_normalization_diagnostic_cases (product_listing_raw_revision_id);

CREATE INDEX product_listing_normalization_diagnostic_cases_source_idx
    ON product_listing_normalization_diagnostic_cases (listing_source_id, generation);
