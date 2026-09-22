-- Run inside one transaction after the caller sets the three aura.bootstrap.* password settings.
-- This script never creates publications or replication slots. DMS first start uses a separately approved existing named slot and LSN.

CREATE EXTENSION IF NOT EXISTS pg_trgm WITH SCHEMA public;
CREATE EXTENSION IF NOT EXISTS unaccent WITH SCHEMA public;

DO $$
DECLARE
    runtime_password text := current_setting('aura.bootstrap.runtime_password', true);
    migrator_password text := current_setting('aura.bootstrap.migrator_password', true);
    replication_password text := current_setting('aura.bootstrap.replication_password', true);
BEGIN
    IF runtime_password IS NULL OR runtime_password = ''
        OR migrator_password IS NULL OR migrator_password = ''
        OR replication_password IS NULL OR replication_password = '' THEN
        RAISE EXCEPTION 'required Aura role passwords are unavailable';
    END IF;

    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'aura_runtime') THEN
        CREATE ROLE aura_runtime LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'aura_migrator') THEN
        CREATE ROLE aura_migrator LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'aura_replication') THEN
        CREATE ROLE aura_replication LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT;
    END IF;

    EXECUTE format('ALTER ROLE aura_runtime PASSWORD %L', runtime_password);
    EXECUTE format('ALTER ROLE aura_migrator PASSWORD %L', migrator_password);
    EXECUTE format('ALTER ROLE aura_replication PASSWORD %L', replication_password);
END;
$$;

REVOKE CREATE ON SCHEMA public FROM PUBLIC;
DO $$
BEGIN
    EXECUTE format(
        'GRANT CONNECT ON DATABASE %I TO aura_runtime, aura_migrator, aura_replication',
        current_database()
    );
    EXECUTE format('GRANT CREATE ON DATABASE %I TO aura_migrator', current_database());
END;
$$;
GRANT aura_migrator TO aura_admin;

GRANT USAGE, CREATE ON SCHEMA public TO aura_migrator;
ALTER SCHEMA public OWNER TO aura_migrator;
GRANT rds_replication TO aura_replication;

SET LOCAL ROLE aura_migrator;
GRANT USAGE ON SCHEMA public TO aura_runtime, aura_replication;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO aura_runtime;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO aura_runtime;
GRANT SELECT ON ALL TABLES IN SCHEMA public TO aura_replication;
ALTER DEFAULT PRIVILEGES IN SCHEMA public
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO aura_runtime;
ALTER DEFAULT PRIVILEGES IN SCHEMA public
    GRANT USAGE, SELECT ON SEQUENCES TO aura_runtime;
ALTER DEFAULT PRIVILEGES IN SCHEMA public
    GRANT SELECT ON TABLES TO aura_replication;
