-- Retires DMS capture: the delivery outbox (20261118090000_delivery_outbox.sql) replaces it on
-- the self-hosted host. Applied there at the cutover; never on RDS, whose DMS task still
-- captures until then.
--
-- DMS kept its heartbeat table in this schema, owned by the replication role. The schema's
-- owner drops it with everything it contains.
DROP SCHEMA IF EXISTS dms_heartbeat CASCADE;

-- The replication role is cluster-wide and belongs to the role bootstrap, not to this
-- database. Drop it here once nothing outside this database still refers to it: the host
-- serves one business database, while a shared test cluster migrates several databases that
-- each grant the role and must keep it. Dropping a role takes an administrator; a migration
-- running without that privilege leaves the role and warns, and the operator drops it (see
-- docs/durable-worker-runbook.md#delivery-outbox-recovery).
DO $$
DECLARE
    replication oid := (SELECT oid FROM pg_roles WHERE rolname = 'aura_replication');
BEGIN
    IF replication IS NULL OR EXISTS (
        SELECT 1
        FROM pg_shdepend
        WHERE refclassid = 'pg_authid'::regclass
          AND refobjid = replication
          AND dbid NOT IN (0, (SELECT oid FROM pg_database WHERE datname = current_database()))
    ) THEN
        RETURN;
    END IF;
    BEGIN
        DROP OWNED BY aura_replication;
        DROP ROLE aura_replication;
    EXCEPTION WHEN insufficient_privilege THEN
        RAISE WARNING 'aura_replication was kept: dropping it needs an administrator';
    END;
END
$$;
