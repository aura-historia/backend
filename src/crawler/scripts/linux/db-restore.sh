#!/usr/bin/env bash
set -euo pipefail

source "$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/db-common.sh"

if [[ $# -ne 2 || "$2" != "--yes" ]]; then
  echo "Usage: $0 <backup-dir> --yes" >&2
  echo "Restores local crawler dev Postgres only." >&2
  exit 1
fi

backup_dir="$1"
ensure_local_crawler_compose

for db_name in "${DB_NAMES[@]}"; do
  if [[ ! -s "${backup_dir}/${db_name}.dump" ]]; then
    echo "Missing backup dump: ${backup_dir}/${db_name}.dump" >&2
    exit 1
  fi
done

"$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/db-up.sh"
container_id="$(postgres_container_id)"

echo "Restoring local crawler dev Postgres only."
for db_name in "${DB_NAMES[@]}"; do
  dump_file="/tmp/${db_name}.dump"
  echo "Restoring ${db_name}"
  docker cp "${backup_dir}/${db_name}.dump" "${container_id}:${dump_file}"
  docker compose -f "${COMPOSE_FILE}" exec -T postgres pg_restore \
    --username postgres \
    --dbname "${db_name}" \
    --clean \
    --if-exists \
    "${dump_file}"
  docker compose -f "${COMPOSE_FILE}" exec -T postgres rm -f "${dump_file}"
done

echo "Crawler Postgres restore complete."
