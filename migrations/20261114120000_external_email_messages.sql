-- External email: transactional email to an address that does not belong to a user account.
-- A producer inserts the message and its notification_deliveries row in its own business
-- transaction; the existing notification delivery pipeline sends it. See
-- docs/events/flow.md (External email delivery).
--
-- The table holds personal data (recipient address and template variables). It is not
-- captured by CDC; only the delivery primary key enters Kinesis. Each origin owns the
-- retention of its messages and erases them by origin; deliveries cascade.

CREATE TABLE external_email_messages (
    external_email_message_id uuid PRIMARY KEY,
    recipient_email text NOT NULL,
    language text NOT NULL,
    template text NOT NULL,
    payload_version integer NOT NULL,
    payload jsonb NOT NULL,
    origin_type text NOT NULL,
    origin_id uuid NOT NULL,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT external_email_messages_recipient_email_check CHECK (
        octet_length(recipient_email) BETWEEN 3 AND 320
        AND recipient_email !~ '[[:space:][:cntrl:]<>]'
        AND position('@' IN recipient_email) > 1
    ),
    CONSTRAINT external_email_messages_language_check CHECK (
        language IN (
            'de', 'en', 'fr', 'es', 'it', 'zh', 'pt', 'pl', 'tr', 'nl',
            'cs', 'ja', 'ru', 'ar', 'ko', 'el', 'he', 'th', 'hi'
        )
    ),
    -- One value per supported template kind. TEST_PROBE is the test-only kind that
    -- exercises the mechanism: no producer enqueues it and no template is deployed for it.
    CONSTRAINT external_email_messages_template_check CHECK (template IN ('TEST_PROBE')),
    CONSTRAINT external_email_messages_payload_version_check CHECK (payload_version >= 1),
    CONSTRAINT external_email_messages_payload_check CHECK (jsonb_typeof(payload) = 'object'),
    CONSTRAINT external_email_messages_origin_type_check
        CHECK (origin_type ~ '^[A-Z][A-Z0-9_]{0,63}$'),
    -- One origin cannot enqueue the same email twice.
    CONSTRAINT external_email_messages_origin_template_unique
        UNIQUE (origin_type, origin_id, template)
);

ALTER TABLE notification_deliveries
    ADD COLUMN external_email_message_id uuid UNIQUE
        REFERENCES external_email_messages ON DELETE CASCADE,
    DROP CONSTRAINT notification_delivery_source_check,
    ADD CONSTRAINT notification_delivery_source_check
        CHECK (num_nonnulls(notification_id, digest_id, external_email_message_id) = 1),
    ADD CONSTRAINT notification_deliveries_external_email_target_check CHECK (
        external_email_message_id IS NULL
        OR (channel = 'EMAIL' AND target_key = 'RECIPIENT')
    );
