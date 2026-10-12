CREATE SCHEMA IF NOT EXISTS dms_heartbeat;
REVOKE ALL ON SCHEMA dms_heartbeat FROM PUBLIC;
GRANT USAGE, CREATE ON SCHEMA dms_heartbeat TO aura_replication;
