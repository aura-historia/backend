-- A listing's vector keeps serving until a new one replaces it, and an embedding revision whose
-- title, description and first image do not change meaningfully confirms it instead of being
-- embedded again. The input behind each vector is recorded here to decide that. See
-- docs/product-listing.md#embeddings.

CREATE TABLE product_listing_embedding_inputs (
    product_listing_id uuid PRIMARY KEY
        REFERENCES product_listings(product_listing_id) ON DELETE CASCADE,
    -- The latest embedding revision the vector stands for: embedded, or confirmed later.
    source_event_id uuid NOT NULL,
    -- The model and request format the vector was computed with; an input of another version
    -- counts as none.
    input_version smallint NOT NULL,
    title_text text NOT NULL,
    description_text text,
    -- The latest URL serving the embedded first image; NULL when the vector has no image.
    image_url text,
    -- The image's perceptual hash; NULL for an image embedded without being decoded.
    image_hash bytea,
    CONSTRAINT product_listing_embedding_inputs_image_hash_check CHECK (
        image_hash IS NULL OR (image_url IS NOT NULL AND octet_length(image_hash) = 64)
    )
);

-- Until now every new embedding revision cleared the vector, so a stored vector stands for its
-- listing's current revision and was computed from its current text and first image. Whether
-- that image could be sent then, and its hash, are unknown: it is recorded without a hash, so a
-- new image URL embeds the listing again.
INSERT INTO product_listing_embedding_inputs (
    product_listing_id, source_event_id, input_version, title_text, description_text, image_url
)
SELECT product_listing_id, embedding_source_event_id, 1, title_text, description_text,
       product_images -> 0 ->> 'url'
FROM product_listings
WHERE embedding IS NOT NULL AND title_text IS NOT NULL;

-- The input version a batch is submitted with; results of another version are submitted again.
-- Code that does not set it embeds version 1.
ALTER TABLE product_listing_embedding_batches
    ADD COLUMN input_version smallint NOT NULL DEFAULT 1;

-- Whether a submitted request embeds its first image, and that image's hash, until its result
-- records them with the vector.
ALTER TABLE product_listing_embedding_work
    ADD COLUMN image_staged boolean NOT NULL DEFAULT false,
    ADD COLUMN image_hash bytea,
    ADD CONSTRAINT product_listing_embedding_work_staged_image_check CHECK (
        (NOT image_staged OR batch_id IS NOT NULL)
        AND (image_hash IS NULL OR (image_staged AND octet_length(image_hash) = 64))
    );
