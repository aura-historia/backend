-- Durable PostgreSQL job queue (src/platform-postgres-queue/README.md). It replaces the SQS
-- worker queues, the two FIFO queues and their dead-letter queues once the runtime moves to
-- the self-hosted host; until then nothing reads or writes these tables.
--
-- A job lives in job_queue_jobs until a worker acknowledges it (DELETE) or it moves, unchanged,
-- into job_queue_failures (failure custody). Operators redrive custody back into the queue; no
-- runtime path ever deletes a custody row without reinserting its job.
--
-- Neither table is a CDC source, and both hold job payloads that must not be logged.

---------------------------------------------------------------------------------------------
-- Queued and leased jobs
---------------------------------------------------------------------------------------------

CREATE TABLE job_queue_jobs (
    job_id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    -- Enqueue order. FIFO scopes release the lowest sequence of a group first, and redrive
    -- keeps the original value so a redriven job keeps its place.
    sequence bigint GENERATED ALWAYS AS IDENTITY,
    scope text NOT NULL,
    -- FIFO scopes only: jobs of one group run one at a time, oldest first.
    group_key text,
    -- Unique per scope among jobs no worker has claimed yet.
    dedup_key text,
    -- The unchanged job body, exactly as enqueued.
    payload text NOT NULL,
    enqueued_at timestamptz NOT NULL DEFAULT now(),
    -- The job can be claimed once this has passed: on enqueue, after a retry's backoff and
    -- when a lease expires (a claim sets it to leased_until).
    available_at timestamptz NOT NULL DEFAULT now(),
    attempts integer NOT NULL DEFAULT 0,
    lease_token uuid,
    leased_until timestamptz,
    last_claimed_at timestamptz,
    -- Bounded class of the last failed attempt; never an error message or payload content.
    last_error_class text,
    CONSTRAINT job_queue_jobs_scope_check CHECK (scope ~ '^[a-z0-9]+(-[a-z0-9]+)*$'
        AND octet_length(scope) <= 48),
    CONSTRAINT job_queue_jobs_group_key_check CHECK (group_key ~ '^[!-~]{1,128}$'),
    CONSTRAINT job_queue_jobs_dedup_key_check CHECK (dedup_key ~ '^[!-~]{1,128}$'),
    CONSTRAINT job_queue_jobs_payload_check CHECK (octet_length(payload) BETWEEN 1 AND 262144),
    CONSTRAINT job_queue_jobs_attempts_check CHECK (attempts >= 0),
    CONSTRAINT job_queue_jobs_lease_check
        CHECK ((lease_token IS NULL) = (leased_until IS NULL)),
    CONSTRAINT job_queue_jobs_error_class_check
        CHECK (last_error_class ~ '^[a-z0-9_]{1,64}$')
);

-- Standard claim scan: the oldest claimable jobs of one scope. Claims pin their plans to these
-- ordered index scans, because statistics of a table that swings between empty and a large
-- backlog are routinely stale.
CREATE INDEX job_queue_jobs_claim_idx ON job_queue_jobs (scope, available_at)
    WHERE group_key IS NULL;

-- FIFO claim walk in enqueue order. It stops after the batch's heads, so its cost grows with
-- the jobs queued behind a blocked head, not with the queue: groups are meant to be small.
CREATE INDEX job_queue_jobs_fifo_idx ON job_queue_jobs (scope, sequence)
    WHERE group_key IS NOT NULL;

-- FIFO head check: is there an older job in the group?
CREATE INDEX job_queue_jobs_group_idx ON job_queue_jobs (scope, group_key, sequence)
    WHERE group_key IS NOT NULL;

-- FIFO lease check: is any job of the group leased? Only leased rows are indexed.
CREATE INDEX job_queue_jobs_group_lease_idx ON job_queue_jobs (scope, group_key)
    WHERE group_key IS NOT NULL AND lease_token IS NOT NULL;

-- Enqueue is insert-or-nothing against jobs that no worker has claimed yet. A claimed job
-- (attempts > 0) leaves the index, so a retry never collides with a newer duplicate.
CREATE UNIQUE INDEX job_queue_jobs_dedup_idx ON job_queue_jobs (scope, dedup_key)
    WHERE dedup_key IS NOT NULL AND attempts = 0;

-- Every claim, retry and acknowledgment rewrites or deletes a row, so dead tuples accumulate
-- quickly while the live table stays small. Vacuum and analyze after a fixed number of changes
-- instead of a share of the table, and without cost-based throttling, so scans of a mostly
-- empty queue never wade through dead rows.
ALTER TABLE job_queue_jobs SET (
    fillfactor = 80,
    autovacuum_vacuum_scale_factor = 0,
    autovacuum_vacuum_threshold = 5000,
    autovacuum_vacuum_insert_scale_factor = 0,
    autovacuum_vacuum_insert_threshold = 5000,
    autovacuum_analyze_scale_factor = 0,
    autovacuum_analyze_threshold = 5000,
    autovacuum_vacuum_cost_delay = 0
);

-- Wakes the workers listening on the scope's channel once the enqueueing transaction
-- commits. PostgreSQL folds identical notifications of one transaction into one.
CREATE FUNCTION aura_job_queue_notify()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    PERFORM pg_notify('job_queue:' || scope, '')
    FROM (SELECT DISTINCT scope FROM inserted_jobs) AS scopes;
    RETURN NULL;
END
$$;

CREATE TRIGGER job_queue_jobs_notify
    AFTER INSERT ON job_queue_jobs
    REFERENCING NEW TABLE AS inserted_jobs
    FOR EACH STATEMENT
    EXECUTE FUNCTION aura_job_queue_notify();

---------------------------------------------------------------------------------------------
-- Failure custody
---------------------------------------------------------------------------------------------

-- A job whose retry budget ran out, or that its consumer parked, keeps its identity, order,
-- grouping and payload here until an operator redrives it. Rows are never purged.
CREATE TABLE job_queue_failures (
    job_id uuid PRIMARY KEY,
    sequence bigint NOT NULL,
    scope text NOT NULL,
    group_key text,
    dedup_key text,
    payload text NOT NULL,
    enqueued_at timestamptz NOT NULL,
    attempts integer NOT NULL,
    reason text NOT NULL,
    last_error_class text,
    last_claimed_at timestamptz,
    failed_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT job_queue_failures_reason_check
        CHECK (reason IN ('RETRY_BUDGET_EXHAUSTED', 'PARKED'))
);

-- Inspection counts and ordered redrive per scope.
CREATE INDEX job_queue_failures_scope_idx ON job_queue_failures (scope, sequence);

-- Custody changes only when jobs fail or are redriven; the default autovacuum thresholds fit.
