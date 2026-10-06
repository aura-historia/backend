-- A repair is operational compensation, never new user consent evidence.
ALTER TABLE marketing_email_consent_sync_intents
    DROP CONSTRAINT marketing_email_consent_sync_intents_subject_check,
    ADD CONSTRAINT marketing_email_consent_sync_intents_subject_check CHECK (
        (subject_type = 'USER' AND user_id IS NOT NULL AND consent_revision > 0
            AND source IN ('COGNITO_SIGNUP', 'AURA_DOUBLE_OPT_IN', 'USER_WITHDRAWAL', 'USER_DELETION', 'PROVIDER_RACE_REPAIR'))
        OR (subject_type = 'EMAIL_ONLY' AND user_id IS NULL AND consent_revision IS NULL
            AND source IN ('AURA_DOUBLE_OPT_IN', 'EMAIL_ONLY_WITHDRAWAL', 'PROVIDER_RACE_REPAIR'))
    ),
    ADD CONSTRAINT marketing_email_consent_sync_intents_repair_revoke_check CHECK (
        source <> 'PROVIDER_RACE_REPAIR' OR NOT desired
    );
