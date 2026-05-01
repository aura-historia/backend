#!/usr/bin/env bash
set -euo pipefail

source "$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/db-common.sh"

for db_url in "${DB_URLS[@]}"; do
  echo "Migration status for ${db_url}"
  cargo sqlx migrate info --source "${MIGRATIONS_DIR}" --database-url "${db_url}"
done
