-- One connection per user and metadata document client groups that client's credentials:
-- its access tokens and the refresh-token families of its sign-ins. Ending a connection
-- cascades to all of them. See docs/oauth.md.
CREATE TABLE oauth_connections (
    connection_id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    client_id text NOT NULL,
    client_name text NOT NULL,
    scopes text[] NOT NULL,
    created timestamptz NOT NULL,
    last_used timestamptz NOT NULL,
    CONSTRAINT oauth_connections_user_client_unique UNIQUE (user_id, client_id),
    -- Lets access tokens reference their connection together with its user and client.
    CONSTRAINT oauth_connections_identity_unique UNIQUE (connection_id, user_id, client_id),
    CONSTRAINT oauth_connections_last_used_check CHECK (last_used >= created),
    CONSTRAINT oauth_connections_scopes_check CHECK (
        scopes <@ ARRAY[
            'users:read',
            'users:write',
            'search-filters:read',
            'search-filters:write',
            'watchlist:read',
            'watchlist:write',
            'notifications:read',
            'notifications:write'
        ]::text[]
    )
);

-- The rotation chain of one sign-in. expires_at is the current token's idle limit, never
-- beyond the family's absolute limit.
CREATE TABLE oauth_refresh_token_families (
    family_id uuid PRIMARY KEY,
    connection_id uuid NOT NULL REFERENCES oauth_connections(connection_id) ON DELETE CASCADE,
    scopes text[] NOT NULL,
    resource text NOT NULL,
    created timestamptz NOT NULL,
    absolute_expires_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    CONSTRAINT oauth_refresh_token_families_lifetime_check CHECK (
        created < absolute_expires_at AND expires_at <= absolute_expires_at
    ),
    CONSTRAINT oauth_refresh_token_families_resource_check CHECK (
        resource IN (
            'https://api.aura-historia.com',
            'https://mcp.aura-historia.com/v1',
            'https://api.stage.aura-historia.com',
            'https://mcp.stage.aura-historia.com/v1'
        )
    ),
    CONSTRAINT oauth_refresh_token_families_scopes_check CHECK (
        scopes <@ ARRAY[
            'users:read',
            'users:write',
            'search-filters:read',
            'search-filters:write',
            'watchlist:read',
            'watchlist:write',
            'notifications:read',
            'notifications:write'
        ]::text[]
    )
);

-- Every token a family issued. Replaced tokens stay until the family ends, so presenting
-- any of them again is recognized as reuse.
CREATE TABLE oauth_refresh_tokens (
    token_short text NOT NULL,
    token_hash text NOT NULL,
    family_id uuid NOT NULL REFERENCES oauth_refresh_token_families(family_id) ON DELETE CASCADE,
    issued timestamptz NOT NULL,
    replaced_at timestamptz,
    PRIMARY KEY (token_short, token_hash),
    CONSTRAINT oauth_refresh_tokens_replaced_check CHECK (
        replaced_at IS NULL OR replaced_at >= issued
    )
);

CREATE UNIQUE INDEX oauth_refresh_tokens_one_current_idx
    ON oauth_refresh_tokens (family_id) WHERE replaced_at IS NULL;
CREATE INDEX oauth_refresh_tokens_family_idx ON oauth_refresh_tokens (family_id);
CREATE INDEX oauth_refresh_token_families_connection_idx
    ON oauth_refresh_token_families (connection_id);
CREATE INDEX oauth_refresh_token_families_expires_at_idx
    ON oauth_refresh_token_families (expires_at);

-- Metadata document client tokens issued before connections existed expire within an hour
-- and cannot name a connection; they are revoked instead of migrated.
DELETE FROM access_tokens WHERE origin = 'OAUTH_METADATA_DOCUMENT';

ALTER TABLE access_tokens
    ADD COLUMN oauth_connection_id uuid,
    DROP CONSTRAINT access_tokens_oauth_origin_client_check,
    ADD CONSTRAINT access_tokens_oauth_origin_client_check CHECK (
        (origin = 'USER'
            AND oauth_client_id IS NULL
            AND oauth_metadata_document_client_id IS NULL
            AND oauth_connection_id IS NULL)
        OR (origin = 'OAUTH'
            AND oauth_client_id IS NOT NULL
            AND oauth_metadata_document_client_id IS NULL
            AND oauth_connection_id IS NULL)
        OR (origin = 'OAUTH_METADATA_DOCUMENT'
            AND oauth_client_id IS NULL
            AND oauth_metadata_document_client_id IS NOT NULL
            AND oauth_connection_id IS NOT NULL)
    ),
    -- A token belongs to its connection's user and client and never outlives it.
    ADD CONSTRAINT access_tokens_oauth_connection_fkey
        FOREIGN KEY (oauth_connection_id, user_id, oauth_metadata_document_client_id)
        REFERENCES oauth_connections (connection_id, user_id, client_id)
        ON DELETE CASCADE;

CREATE INDEX access_tokens_oauth_connection_id_idx
    ON access_tokens (oauth_connection_id) WHERE oauth_connection_id IS NOT NULL;

ALTER TABLE oauth_authorization_codes
    ADD COLUMN issues_refresh_token boolean NOT NULL DEFAULT false,
    ADD CONSTRAINT oauth_authorization_codes_refresh_token_client_check CHECK (
        NOT issues_refresh_token OR metadata_document_client_id IS NOT NULL
    );

CREATE INDEX oauth_authorization_codes_metadata_document_client_idx
    ON oauth_authorization_codes (user_id, metadata_document_client_id)
    WHERE metadata_document_client_id IS NOT NULL;

-- The cleanup additionally removes the tokens of expired refresh-token families, then
-- those families, then connections that hold no credential and no outstanding
-- authorization code. Every target is still bounded by batch_size and skips locked rows.
DROP FUNCTION cleanup_expired_credentials_and_provider_receipts(integer);

CREATE FUNCTION cleanup_expired_credentials_and_provider_receipts(batch_size integer)
RETURNS TABLE (
    access_tokens_deleted bigint,
    authorization_codes_deleted bigint,
    third_party_exchange_codes_deleted bigint,
    provider_receipts_deleted bigint,
    refresh_tokens_deleted bigint,
    refresh_token_families_deleted bigint,
    oauth_connections_deleted bigint
)
LANGUAGE plpgsql
AS $$
DECLARE
    cleanup_now timestamptz := statement_timestamp();
BEGIN
    IF batch_size IS NULL OR batch_size < 1 OR batch_size > 1000 THEN
        RAISE EXCEPTION 'batch_size must be a non-null integer between 1 and 1000';
    END IF;

    WITH candidates AS (
        SELECT ctid
        FROM oauth_third_party_exchange_codes
        WHERE expires_at < cleanup_now
        ORDER BY expires_at ASC
        LIMIT batch_size
        FOR UPDATE SKIP LOCKED
    ), deleted AS (
        DELETE FROM oauth_third_party_exchange_codes AS codes
        USING candidates
        WHERE codes.ctid = candidates.ctid
        RETURNING 1
    )
    SELECT count(*) INTO third_party_exchange_codes_deleted FROM deleted;

    WITH candidates AS (
        SELECT access_tokens.ctid
        FROM access_tokens
        WHERE expires_at < cleanup_now
          AND NOT EXISTS (
              SELECT 1
              FROM oauth_third_party_exchange_codes AS codes
              WHERE codes.access_token_id = access_tokens.access_token_id
          )
        ORDER BY expires_at ASC
        LIMIT batch_size
        FOR UPDATE SKIP LOCKED
    ), deleted AS (
        DELETE FROM access_tokens
        USING candidates
        WHERE access_tokens.ctid = candidates.ctid
        RETURNING 1
    )
    SELECT count(*) INTO access_tokens_deleted FROM deleted;

    WITH candidates AS (
        SELECT ctid
        FROM oauth_authorization_codes
        WHERE expires_at < cleanup_now
        ORDER BY expires_at ASC
        LIMIT batch_size
        FOR UPDATE SKIP LOCKED
    ), deleted AS (
        DELETE FROM oauth_authorization_codes AS codes
        USING candidates
        WHERE codes.ctid = candidates.ctid
        RETURNING 1
    )
    SELECT count(*) INTO authorization_codes_deleted FROM deleted;

    WITH candidates AS (
        SELECT ctid
        FROM product_listing_raw_provider_observation_receipts
        WHERE expires_at <= cleanup_now
        ORDER BY expires_at ASC
        LIMIT batch_size
        FOR UPDATE SKIP LOCKED
    ), deleted AS (
        DELETE FROM product_listing_raw_provider_observation_receipts AS receipts
        USING candidates
        WHERE receipts.ctid = candidates.ctid
        RETURNING 1
    )
    SELECT count(*) INTO provider_receipts_deleted FROM deleted;

    -- Tokens first, so deleting a family never cascades to an unbounded number of rows.
    -- Replaced tokens go before any current token, so a family keeps its current token
    -- while replaced ones remain and a refresh with either still finds a whole family.
    WITH candidates AS (
        SELECT tokens.ctid
        FROM oauth_refresh_tokens AS tokens
        JOIN oauth_refresh_token_families AS families USING (family_id)
        WHERE families.expires_at < cleanup_now
        ORDER BY tokens.replaced_at IS NULL, families.expires_at
        LIMIT batch_size
        -- A refresh holds its family's lock; skip that family rather than race its rotation.
        FOR UPDATE OF tokens, families SKIP LOCKED
    ), deleted AS (
        DELETE FROM oauth_refresh_tokens AS tokens
        USING candidates
        WHERE tokens.ctid = candidates.ctid
        RETURNING 1
    )
    SELECT count(*) INTO refresh_tokens_deleted FROM deleted;

    WITH candidates AS (
        SELECT families.ctid
        FROM oauth_refresh_token_families AS families
        WHERE families.expires_at < cleanup_now
          AND NOT EXISTS (
              SELECT 1
              FROM oauth_refresh_tokens AS tokens
              WHERE tokens.family_id = families.family_id
          )
        ORDER BY families.expires_at ASC
        LIMIT batch_size
        FOR UPDATE SKIP LOCKED
    ), deleted AS (
        DELETE FROM oauth_refresh_token_families AS families
        USING candidates
        WHERE families.ctid = candidates.ctid
        RETURNING 1
    )
    SELECT count(*) INTO refresh_token_families_deleted FROM deleted;

    WITH candidates AS (
        SELECT connections.ctid
        FROM oauth_connections AS connections
        WHERE NOT EXISTS (
              SELECT 1
              FROM oauth_refresh_token_families AS families
              WHERE families.connection_id = connections.connection_id
          )
          AND NOT EXISTS (
              SELECT 1
              FROM access_tokens
              WHERE access_tokens.oauth_connection_id = connections.connection_id
          )
          AND NOT EXISTS (
              SELECT 1
              FROM oauth_authorization_codes AS codes
              WHERE codes.user_id = connections.user_id
                AND codes.metadata_document_client_id = connections.client_id
          )
        ORDER BY connections.last_used ASC
        LIMIT batch_size
        FOR UPDATE SKIP LOCKED
    ), deleted AS (
        DELETE FROM oauth_connections AS connections
        USING candidates
        WHERE connections.ctid = candidates.ctid
        RETURNING 1
    )
    SELECT count(*) INTO oauth_connections_deleted FROM deleted;

    RETURN NEXT;
END;
$$;
