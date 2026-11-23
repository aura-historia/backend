-- A product URL retired as the duplicate of a language version of its page: the URLs differ only
-- in directories that name a language and share the rest, which holds the identity
-- (docs/crawler/scheduling-and-retries.md, captured identity). The check is replaced NOT VALID,
-- as it was added, so it holds for every write without scanning the existing rows.
ALTER TABLE listing_source_urls
    DROP CONSTRAINT listing_source_urls_duplicate_evidence_known,
    ADD CONSTRAINT listing_source_urls_duplicate_evidence_known CHECK (
        duplicate_evidence IN (
            'REDIRECT_DESTINATION', 'SPELLING', 'IDENTITY_TOKEN', 'LANGUAGE_DIRECTORY', 'TITLE'
        )
    ) NOT VALID;
