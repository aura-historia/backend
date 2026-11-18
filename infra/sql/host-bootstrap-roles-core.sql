-- Role bootstrap of the self-hosted PostgreSQL. Run inside one transaction as the administrator
-- after the caller sets the three aura.bootstrap.* password settings. An optional fourth,
-- aura.bootstrap.crawler_ingestion_password, lets the crawler's ingestion role log in; without it
-- the role exists but cannot log in. Derived from
-- rds-bootstrap-roles-core.sql without logical replication: the delivery outbox replaced DMS.
--
-- The immutable initial business schema still grants the retired DMS heartbeat schema to
-- aura_replication, so a fresh database needs that role while the migrations run. It is created
-- here only as a placeholder that cannot log in and holds no replication capability;
-- 20261118100000_retire_dms_capture.sql drops the heartbeat schema, and the bootstrap drops the
-- placeholder as the administrator after the migrations, so a repeated bootstrap recreates and
-- drops it again.

CREATE EXTENSION IF NOT EXISTS pg_trgm WITH SCHEMA public;
CREATE EXTENSION IF NOT EXISTS unaccent WITH SCHEMA public;

DO $$
DECLARE
    runtime_password text := current_setting('aura.bootstrap.runtime_password', true);
    migrator_password text := current_setting('aura.bootstrap.migrator_password', true);
    inspection_password text := current_setting('aura.bootstrap.inspection_password', true);
    crawler_ingestion_password text :=
        current_setting('aura.bootstrap.crawler_ingestion_password', true);
BEGIN
    IF runtime_password IS NULL OR runtime_password = ''
        OR migrator_password IS NULL OR migrator_password = ''
        OR inspection_password IS NULL OR inspection_password = '' THEN
        RAISE EXCEPTION 'required Aura role passwords are unavailable';
    END IF;

    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'aura_runtime') THEN
        CREATE ROLE aura_runtime LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'aura_migrator') THEN
        CREATE ROLE aura_migrator LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'aura_inspector') THEN
        CREATE ROLE aura_inspector LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT;
    END IF;
    -- The crawler's only right is EXECUTE on the ingestion enqueue function
    -- (migrations/20261117140000_product_listing_ingestion_admission.sql).
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'aura_crawler_ingestion') THEN
        CREATE ROLE aura_crawler_ingestion NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT
            CONNECTION LIMIT 8;
    END IF;
    -- Bound how long a crawler session can hold an enqueue's group lock.
    ALTER ROLE aura_crawler_ingestion SET statement_timeout = '30s';
    ALTER ROLE aura_crawler_ingestion SET idle_in_transaction_session_timeout = '30s';
    IF crawler_ingestion_password IS NOT NULL AND crawler_ingestion_password <> '' THEN
        EXECUTE format('ALTER ROLE aura_crawler_ingestion LOGIN PASSWORD %L',
            crawler_ingestion_password);
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'aura_replication') THEN
        CREATE ROLE aura_replication NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT;
    END IF;

    ALTER ROLE aura_inspector SET default_transaction_read_only = on;
    EXECUTE format('ALTER ROLE aura_inspector PASSWORD %L', inspection_password);
    EXECUTE format('ALTER ROLE aura_runtime PASSWORD %L', runtime_password);
    EXECUTE format('ALTER ROLE aura_migrator PASSWORD %L', migrator_password);
END;
$$;

REVOKE CREATE ON SCHEMA public FROM PUBLIC;
DO $$
BEGIN
    EXECUTE format(
        'GRANT CONNECT ON DATABASE %I TO aura_runtime, aura_migrator, aura_inspector, aura_crawler_ingestion',
        current_database()
    );
    EXECUTE format('GRANT CREATE ON DATABASE %I TO aura_migrator', current_database());
    -- The administrator may be any role with CREATEROLE that owns the database.
    EXECUTE format('GRANT aura_migrator TO %I', current_user);
END;
$$;

GRANT USAGE, CREATE ON SCHEMA public TO aura_migrator;
ALTER SCHEMA public OWNER TO aura_migrator;

SET LOCAL ROLE aura_migrator;
GRANT USAGE ON SCHEMA public TO aura_runtime, aura_inspector, aura_crawler_ingestion;
DO $$ BEGIN
    IF to_regclass('public._sqlx_migrations') IS NOT NULL THEN
        GRANT SELECT (version, checksum, success) ON public._sqlx_migrations TO aura_inspector;
    END IF;
END $$;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO aura_runtime;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO aura_runtime;
ALTER DEFAULT PRIVILEGES IN SCHEMA public
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO aura_runtime;
ALTER DEFAULT PRIVILEGES IN SCHEMA public
    GRANT USAGE, SELECT ON SEQUENCES TO aura_runtime;
