-- Independent of users: deletion custody and registration fences survive erasure.
CREATE TABLE user_identity_deletions (
    user_id uuid PRIMARY KEY,
    issuer text NOT NULL,
    subject text NOT NULL,
    username text,
    version bigint NOT NULL DEFAULT 0 CHECK (version >= 0),
    created timestamptz NOT NULL DEFAULT now(),
    lease_expires_at timestamptz,
    completed_at timestamptz,
    UNIQUE (issuer, subject),
    CHECK (octet_length(issuer) BETWEEN 1 AND 2048 AND issuer !~ '[[:cntrl:]]'),
    CHECK (octet_length(subject) BETWEEN 1 AND 2048 AND subject !~ '[[:cntrl:]]'),
    CHECK (username IS NULL OR (char_length(username) BETWEEN 1 AND 128 AND username !~ '[[:space:][:cntrl:]]')),
    CHECK (completed_at IS NULL OR username IS NOT NULL)
);

-- Only this immutable request enters CDC, Kinesis and SQS.
CREATE TABLE user_identity_deletion_requests (
    user_id uuid PRIMARY KEY REFERENCES user_identity_deletions(user_id)
);
