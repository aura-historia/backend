-- A needs-repair verdict keeps the review pending and records when and why the reviewer asked
-- for repair. The legacy terminal `NEEDS_REPAIR` status stays valid for reviews closed that way
-- earlier; no verdict writes it any more.
ALTER TABLE crawler_reviews
    ADD COLUMN repair_requested TIMESTAMPTZ,
    ADD COLUMN repair_note      TEXT,
    ADD CONSTRAINT crawler_reviews_repair_request_shape_check CHECK (
        (repair_requested IS NULL AND repair_note IS NULL)
        OR (repair_requested IS NOT NULL AND repair_note IS NOT NULL AND btrim(repair_note) <> '')
    );
