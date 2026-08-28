CREATE TABLE IF NOT EXISTS listing_source_removed_page_schemas (
    listing_source_id UUID PRIMARY KEY REFERENCES listing_sources(listing_source_id) ON DELETE CASCADE,
    removed_page_schema JSONB NOT NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
