-- Public OAuth clients identified by the HTTPS URL of their Client ID Metadata Document have
-- no oauth_clients row. Their authorization codes and access tokens record that URL instead.
ALTER TABLE oauth_authorization_codes
    ALTER COLUMN client_id DROP NOT NULL,
    ADD COLUMN metadata_document_client_id text,
    ADD CONSTRAINT oauth_authorization_codes_one_client_check CHECK (
        (client_id IS NULL) <> (metadata_document_client_id IS NULL)
    );

ALTER TABLE access_tokens
    ADD COLUMN oauth_metadata_document_client_id text,
    DROP CONSTRAINT access_tokens_origin_check,
    DROP CONSTRAINT access_tokens_oauth_origin_client_check,
    ADD CONSTRAINT access_tokens_origin_check CHECK (
        origin IN ('USER', 'OAUTH', 'OAUTH_METADATA_DOCUMENT')
    ),
    ADD CONSTRAINT access_tokens_oauth_origin_client_check CHECK (
        (origin = 'USER'
            AND oauth_client_id IS NULL
            AND oauth_metadata_document_client_id IS NULL)
        OR (origin = 'OAUTH'
            AND oauth_client_id IS NOT NULL
            AND oauth_metadata_document_client_id IS NULL)
        OR (origin = 'OAUTH_METADATA_DOCUMENT'
            AND oauth_client_id IS NULL
            AND oauth_metadata_document_client_id IS NOT NULL)
    );
