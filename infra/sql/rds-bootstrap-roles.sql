\set ON_ERROR_STOP on

-- Break-glass manual equivalent of database-migration-lambda. Run as the generated aura_admin user.
-- Supply values with psql -v runtime_password=... -v migrator_password=... -v replication_password=... -v inspection_password=....
-- Optionally add -v crawler_ingestion_password=... to let the crawler's ingestion role log in.
-- This script never creates publications or replication slots. DMS uses the separately approved pre-existing named slot.

BEGIN;
SELECT set_config('aura.bootstrap.runtime_password', :'runtime_password', true);
SELECT set_config('aura.bootstrap.migrator_password', :'migrator_password', true);
SELECT set_config('aura.bootstrap.replication_password', :'replication_password', true);
SELECT set_config('aura.bootstrap.inspection_password', :'inspection_password', true);
\if :{?crawler_ingestion_password}
SELECT set_config('aura.bootstrap.crawler_ingestion_password', :'crawler_ingestion_password', true);
\endif
\ir rds-bootstrap-roles-core.sql
COMMIT;
