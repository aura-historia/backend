#!/usr/bin/env bash
set -euo pipefail

source "$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/db-common.sh"

for db_url in "${DB_URLS[@]}"; do
  echo "Migrating ${db_url}"
  cargo sqlx migrate run --source "${MIGRATIONS_DIR}" --database-url "${db_url}"
done

echo "Migrations applied to all crawler databases."
