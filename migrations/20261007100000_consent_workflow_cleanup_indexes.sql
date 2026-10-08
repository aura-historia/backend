-- Retention scans use completion times. Leave active/blocked/failed work outside
-- these indexes and outside automatic cleanup.
CREATE INDEX newsletter_subscription_confirmations_confirmed_cleanup_idx
    ON newsletter_subscription_confirmations (confirmed_at, confirmation_id)
    WHERE confirmed_at IS NOT NULL;

CREATE INDEX marketing_email_consent_sync_intents_completed_cleanup_idx
    ON marketing_email_consent_sync_intents (completed_at, intent_id)
    WHERE status IN ('APPLIED', 'SUPERSEDED') AND completed_at IS NOT NULL;

-- Older receipts may carry the previous 35-day expires_at. Their processed_at
-- still controls the minimum 120-day retention window.
CREATE INDEX loops_webhook_receipts_processed_cleanup_idx
    ON loops_webhook_receipts (processed_at, delivery_id);

-- Operator samples remain bounded without a full scan of old completed work.
CREATE INDEX marketing_email_consent_sync_intents_unsettled_diagnostic_idx
    ON marketing_email_consent_sync_intents (status, intent_sequence)
    WHERE status IN ('BLOCKED', 'FAILED');
CREATE INDEX marketing_email_consent_sync_intents_repair_diagnostic_idx
    ON marketing_email_consent_sync_intents (intent_sequence)
    WHERE source = 'PROVIDER_RACE_REPAIR' AND status NOT IN ('APPLIED', 'SUPERSEDED');
CREATE INDEX marketing_email_consent_sync_intents_race_diagnostic_idx
    ON marketing_email_consent_sync_intents (intent_sequence)
    WHERE status = 'BLOCKED' AND last_error_code = 'PROVIDER_WITHDRAWAL_RACE_CANDIDATE';
CREATE INDEX marketing_email_consent_sync_intents_expired_grant_diagnostic_idx
    ON marketing_email_consent_sync_intents (not_after, intent_id)
    WHERE desired AND status IN ('PENDING', 'IN_PROGRESS', 'BLOCKED', 'FAILED');
