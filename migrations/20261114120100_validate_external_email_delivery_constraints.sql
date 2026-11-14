-- Validates the constraints added NOT VALID by 20261114120000_external_email_messages.sql.
-- VALIDATE CONSTRAINT scans existing rows under a lock that does not block delivery writes.
ALTER TABLE notification_deliveries
    VALIDATE CONSTRAINT notification_deliveries_external_email_message_id_fkey;

ALTER TABLE notification_deliveries
    VALIDATE CONSTRAINT notification_delivery_source_check;

ALTER TABLE notification_deliveries
    VALIDATE CONSTRAINT notification_deliveries_external_email_target_check;
