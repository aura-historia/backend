#!/usr/bin/env bash
# Runs once, when the image initializes an empty data directory. Creates the administrator that
# `ops bootstrap` connects as: it creates the application roles and owns the database, but is
# no superuser, as the RDS master user was. The superuser itself never connects over a network
# (pg_hba.conf).
set -euo pipefail
: "${AURA_ADMIN_PASSWORD:?AURA_ADMIN_PASSWORD is required on first initialization}"
# The password is read from the environment inside psql, never passed as an argument.
psql -v ON_ERROR_STOP=1 --username "${POSTGRES_USER}" --dbname "${POSTGRES_DB}" \
  --set=database="${POSTGRES_DB}" <<'SQL'
\getenv password AURA_ADMIN_PASSWORD
CREATE ROLE aura_admin LOGIN CREATEROLE NOSUPERUSER NOCREATEDB NOREPLICATION NOBYPASSRLS
    PASSWORD :'password';
ALTER DATABASE :"database" OWNER TO aura_admin;
SQL
