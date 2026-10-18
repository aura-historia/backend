ALTER TABLE user_cognito_identities
    ADD COLUMN sessions_revoked_before timestamptz,
    ADD CONSTRAINT user_cognito_identities_session_cutoff_valid CHECK (
        sessions_revoked_before IS NULL OR (
            isfinite(sessions_revoked_before)
            AND sessions_revoked_before >= TIMESTAMPTZ '1970-01-01 00:00:00+00'
        )
    );
