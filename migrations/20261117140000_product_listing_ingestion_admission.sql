-- ProductListing ingestion admission through the job queue (docs/product-listing.md#asynchronous-ingestion-submission).
-- Every producer (partner API, provider webhooks, crawler) admits commands through one
-- SECURITY DEFINER function, so the crawler's database role needs no table privilege at all.

-- Ingestion commands keep their wire codec's 1 MiB message limit.
ALTER TABLE job_queue_jobs
    DROP CONSTRAINT job_queue_jobs_payload_check,
    ADD CONSTRAINT job_queue_jobs_payload_check CHECK (octet_length(payload) BETWEEN 1 AND 1048576);

-- Enqueues one command into the product-listing-ingestion FIFO scope and returns its job ID,
-- or NULL when an unclaimed job already carries the dedup key. It mirrors
-- platform_postgres_queue::enqueue: the group's transaction lock (same key) keeps a group's jobs
-- visible in enqueue order. Arguments are validated here because callers hold no other right.
CREATE FUNCTION job_queue_enqueue_product_listing_ingestion(
    p_group_key text,
    p_dedup_key text,
    p_payload text
)
RETURNS uuid
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $$
DECLARE
    v_scope constant text := 'product-listing-ingestion';
    v_job_id uuid;
BEGIN
    IF p_group_key IS NULL OR p_group_key !~ '^[!-~]{1,128}$'
        OR p_dedup_key IS NULL OR p_dedup_key !~ '^[!-~]{1,128}$' THEN
        RAISE EXCEPTION 'ingestion job keys must be 1 to 128 bytes of printable ASCII'
            USING ERRCODE = 'invalid_parameter_value';
    END IF;
    IF p_payload IS NULL OR octet_length(p_payload) NOT BETWEEN 1 AND 1048576 THEN
        RAISE EXCEPTION 'ingestion job payload must be 1 to 1048576 bytes'
            USING ERRCODE = 'invalid_parameter_value';
    END IF;

    PERFORM pg_advisory_xact_lock(
        hashtextextended('job_queue:' || v_scope || ':' || p_group_key, 0));
    INSERT INTO public.job_queue_jobs AS jobs (scope, group_key, dedup_key, payload)
    VALUES (v_scope, p_group_key, p_dedup_key, p_payload)
    ON CONFLICT (scope, dedup_key) WHERE dedup_key IS NOT NULL AND attempts = 0
    DO NOTHING
    RETURNING jobs.job_id INTO v_job_id;
    RETURN v_job_id;
END
$$;

-- Functions are executable by PUBLIC by default. aura_crawler_ingestion is deployment-managed
-- like aura_replication and holds this grant only.
REVOKE ALL ON FUNCTION job_queue_enqueue_product_listing_ingestion(text, text, text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION job_queue_enqueue_product_listing_ingestion(text, text, text)
    TO aura_runtime, aura_crawler_ingestion;
