-- A human's request that the live schema audit reopen the ListingSource for the review agents
-- (#3118; docs/crawler/extraction-and-review.md, live schema audit), for what no check finds, such
-- as titles that dropped the dating the shop shows. The note says what to repair; the audit clears
-- both once it opens the review.
ALTER TABLE listing_sources
    ADD COLUMN schema_audit_requested_at TIMESTAMPTZ,
    ADD COLUMN schema_audit_request_note TEXT,
    ADD CONSTRAINT listing_sources_schema_audit_request_check
        CHECK ((schema_audit_requested_at IS NULL) = (schema_audit_request_note IS NULL));
