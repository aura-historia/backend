-- Product embeddings run in provider batches instead of one synchronous call per listing. A
-- listing's embedding revision waits here between its embedding job and its committed
-- embedding: pending, assigned to a batch, or parked after spending its attempts. One row per
-- listing holds its latest revision; a newer revision replaces it. See
-- docs/product-listing.md#embeddings.

CREATE TABLE product_listing_embedding_batches (
    batch_id uuid PRIMARY KEY,
    -- Unknown until the provider accepted the batch.
    provider_batch_id text UNIQUE,
    created timestamptz NOT NULL DEFAULT now(),
    submitted timestamptz,
    CONSTRAINT product_listing_embedding_batches_submission_check
        CHECK ((provider_batch_id IS NULL) = (submitted IS NULL))
);

CREATE TABLE product_listing_embedding_work (
    product_listing_id uuid PRIMARY KEY
        REFERENCES product_listings(product_listing_id) ON DELETE CASCADE,
    source_event_id uuid NOT NULL,
    attempts smallint NOT NULL DEFAULT 0,
    parked timestamptz,
    batch_id uuid REFERENCES product_listing_embedding_batches(batch_id),
    batch_position integer,
    enqueued timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT product_listing_embedding_work_attempts_check CHECK (attempts >= 0),
    CONSTRAINT product_listing_embedding_work_assignment_check CHECK (
        (batch_id IS NULL AND batch_position IS NULL)
        OR (batch_id IS NOT NULL AND batch_position > 0 AND parked IS NULL)
    )
);

CREATE INDEX product_listing_embedding_work_pending_idx
    ON product_listing_embedding_work (enqueued)
    WHERE batch_id IS NULL AND parked IS NULL;

CREATE INDEX product_listing_embedding_work_batch_idx
    ON product_listing_embedding_work (batch_id, batch_position)
    WHERE batch_id IS NOT NULL;
