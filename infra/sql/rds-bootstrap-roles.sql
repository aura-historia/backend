\set ON_ERROR_STOP on

-- Run once as the generated aura_admin user after the RDS instance is ready.
-- Supply values with psql -v runtime_password=... -v migrator_password=... -v replication_password=....
-- This script never creates publications or replication slots. The DMS source task owns both.

BEGIN;

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'aura_runtime') THEN
        CREATE ROLE aura_runtime LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'aura_migrator') THEN
        CREATE ROLE aura_migrator LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'aura_replication') THEN
        CREATE ROLE aura_replication LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT;
    END IF;
END;
$$;

ALTER ROLE aura_runtime PASSWORD :'runtime_password';
ALTER ROLE aura_migrator PASSWORD :'migrator_password';
ALTER ROLE aura_replication PASSWORD :'replication_password';

REVOKE CREATE ON SCHEMA public FROM PUBLIC;
GRANT CONNECT ON DATABASE aura_historia TO aura_runtime, aura_migrator, aura_replication;

GRANT USAGE, CREATE ON SCHEMA public TO aura_migrator;
ALTER SCHEMA public OWNER TO aura_migrator;

GRANT USAGE ON SCHEMA public TO aura_runtime, aura_replication;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO aura_runtime;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO aura_runtime;
ALTER DEFAULT PRIVILEGES FOR ROLE aura_migrator IN SCHEMA public
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO aura_runtime;
ALTER DEFAULT PRIVILEGES FOR ROLE aura_migrator IN SCHEMA public
    GRANT USAGE, SELECT ON SEQUENCES TO aura_runtime;

GRANT SELECT ON ALL TABLES IN SCHEMA public TO aura_replication;
ALTER DEFAULT PRIVILEGES FOR ROLE aura_migrator IN SCHEMA public
    GRANT SELECT ON TABLES TO aura_replication;
GRANT rds_replication TO aura_replication;

COMMIT;
