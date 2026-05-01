#!/usr/bin/env bash
set -euo pipefail

source "$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/db-common.sh"

docker compose -f "${COMPOSE_FILE}" up -d

for db_url in "${DB_URLS[@]}"; do
  cargo sqlx database create --database-url "${db_url}"
done

echo "Local crawler Postgres is up and databases are ready."
