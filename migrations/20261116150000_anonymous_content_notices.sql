-- Content notices: anonymous reports of illegal content and the submitter role OTHER
-- (docs/content-notices.md).
--
-- DSA Recital 50 lets a notifier stay unidentified and Art. 16(2)(c) exempts reports of child
-- sexual abuse offences from naming the notifier, so a report of illegal content may omit the
-- name and the email address. Every other role still has to give both. The existing value
-- checks on name and email hold for the values that are present; a null passes them.
--
-- content_notices is not captured by CDC. The statement validates the new constraint against
-- every stored notice under its exclusive lock; every stored notice has a name and an email.
ALTER TABLE content_notices
    ALTER COLUMN submitter_name DROP NOT NULL,
    ALTER COLUMN submitter_email DROP NOT NULL,
    -- A notice without an email address has no address key; the submission limits group it
    -- with the other notices without an address about its target.
    ALTER COLUMN submitter_email_key DROP NOT NULL,
    DROP CONSTRAINT content_notices_submitter_role_check,
    ADD CONSTRAINT content_notices_submitter_role_check CHECK (
        submitter_role IN ('PROVIDER', 'RIGHTS_HOLDER', 'ILLEGAL_CONTENT_REPORTER', 'OTHER')
    ),
    ADD CONSTRAINT content_notices_submitter_identification_check CHECK (
        submitter_role = 'ILLEGAL_CONTENT_REPORTER'
        OR (submitter_name IS NOT NULL AND submitter_email IS NOT NULL)
    );
