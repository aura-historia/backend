ALTER TABLE oauth_authorization_codes
    ADD COLUMN resource TEXT,
    ADD CONSTRAINT oauth_authorization_code_resource_valid CHECK (
        resource IS NULL OR resource IN (
            'https://api.aura-historia.com',
            'https://mcp.aura-historia.com/v1',
            'https://api.stage.aura-historia.com',
            'https://mcp.stage.aura-historia.com/v1'
        )
    );

COMMENT ON COLUMN oauth_authorization_codes.resource IS
    'Exact Aura resource identifier approved for this code; NULL legacy codes use the configured issuer shared-resource default.';
