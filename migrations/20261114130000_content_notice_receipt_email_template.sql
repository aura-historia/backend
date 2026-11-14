-- Adds the content notice receipt (docs/content-notices.md, Receipt email) to the external
-- email template kinds. external_email_messages is small and not captured by CDC, so the
-- re-added CHECK validates its rows under a brief lock.
ALTER TABLE external_email_messages
    DROP CONSTRAINT external_email_messages_template_check,
    ADD CONSTRAINT external_email_messages_template_check
        CHECK (template IN ('TEST_PROBE', 'CONTENT_NOTICE_RECEIPT'));
