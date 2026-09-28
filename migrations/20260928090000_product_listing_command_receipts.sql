-- Operational idempotency state for successfully completed per-command ingestion execution.
-- No FK to mutable business rows: receipts must survive listing/source deletion.
CREATE TABLE product_listing_command_receipts (
    command_id text PRIMARY KEY,
    submission_id text NOT NULL,
    listing_source_id uuid NOT NULL,
    operation text NOT NULL,
    semantic_fingerprint bytea NOT NULL,
    completion_code text NOT NULL,
    completed_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    CONSTRAINT product_listing_command_receipts_command_id_check
        CHECK (command_id ~ '^plic1_[0-9a-f]{64}$'),
    CONSTRAINT product_listing_command_receipts_submission_id_check
        CHECK (submission_id ~ '^plis1_[0-9a-f]{64}$'),
    CONSTRAINT product_listing_command_receipts_operation_check
        CHECK (operation IN ('CREATE', 'UPDATE', 'UPSERT', 'WITHDRAW', 'CAPTURE_RAW')),
    CONSTRAINT product_listing_command_receipts_fingerprint_check
        CHECK (octet_length(semantic_fingerprint) = 32),
    CONSTRAINT product_listing_command_receipts_completion_code_check
        CHECK (completion_code = 'APPLIED')
);
