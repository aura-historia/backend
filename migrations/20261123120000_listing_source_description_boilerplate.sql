-- Reviewed description boilerplate per listing source (docs/product-listing.md, description
-- boilerplate). A STRIP sentence is removed from the source's descriptions during raw
-- normalization; a KEEP sentence was reviewed as an item fact and is never proposed again.
-- `sentence` holds the normalized sentence key the normalizer matches on.
CREATE TABLE listing_source_description_boilerplate (
    listing_source_id uuid NOT NULL REFERENCES listing_sources(listing_source_id) ON DELETE CASCADE,
    sentence text NOT NULL,
    decision text NOT NULL,
    reviewed_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (listing_source_id, sentence),
    CONSTRAINT listing_source_description_boilerplate_decision_check
        CHECK (decision IN ('STRIP', 'KEEP')),
    CONSTRAINT listing_source_description_boilerplate_sentence_bytes_check
        CHECK (octet_length(sentence) BETWEEN 1 AND 1000)
);

-- Provenance: the order-independent SHA-256 of the STRIP set a completion normalized with, or
-- NULL when the source had none.
ALTER TABLE product_listing_raw_normalizations
    ADD COLUMN description_boilerplate_sha256 bytea,
    ADD CONSTRAINT product_listing_raw_normalizations_boilerplate_sha256_check
        CHECK (description_boilerplate_sha256 IS NULL OR octet_length(description_boilerplate_sha256) = 32);
