ALTER TABLE users
  ADD COLUMN marketing_email_consent boolean NOT NULL DEFAULT false,
  ADD COLUMN marketing_email_consent_revision bigint NOT NULL DEFAULT 0,
  ADD CONSTRAINT users_marketing_email_consent_revision_nonnegative
    CHECK (marketing_email_consent_revision >= 0);
