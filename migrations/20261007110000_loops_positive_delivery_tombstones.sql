-- A positive webhook must remain spent after its detailed receipt expires. Keep
-- only the opaque delivery ID and exact-body digest; no mailbox or provider state.
CREATE TABLE loops_webhook_positive_delivery_tombstones (
    delivery_id text PRIMARY KEY,
    raw_body_sha256 bytea NOT NULL,
    CONSTRAINT loops_positive_delivery_id_check
        CHECK (octet_length(delivery_id) BETWEEN 1 AND 256 AND delivery_id !~ '[[:cntrl:]]'),
    CONSTRAINT loops_positive_delivery_digest_check
        CHECK (octet_length(raw_body_sha256) = 32)
);

-- Block old writers while copying existing positive receipts and installing the
-- transactional marker. This also covers writes from the old application during
-- the deployment interval before all webhook workers use the new lookup.
LOCK TABLE loops_webhook_receipts IN SHARE ROW EXCLUSIVE MODE;
INSERT INTO loops_webhook_positive_delivery_tombstones (delivery_id, raw_body_sha256)
SELECT delivery_id, raw_body_sha256 FROM loops_webhook_receipts
WHERE provider_event_name = 'email.resubscribed';

CREATE FUNCTION record_loops_positive_delivery() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.provider_event_name = 'email.resubscribed' THEN
        INSERT INTO loops_webhook_positive_delivery_tombstones (delivery_id, raw_body_sha256)
        VALUES (NEW.delivery_id, NEW.raw_body_sha256)
        ON CONFLICT (delivery_id) DO NOTHING;
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER record_loops_positive_delivery
AFTER INSERT ON loops_webhook_receipts
FOR EACH ROW EXECUTE FUNCTION record_loops_positive_delivery();
