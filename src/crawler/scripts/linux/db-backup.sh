#!/usr/bin/env bash
set -euo pipefail

source "$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/db-common.sh"

ensure_local_crawler_compose
docker compose -f "${COMPOSE_FILE}" up -d

timestamp="$(date -u +"%Y%m%dT%H%M%SZ")"
backup_dir="${1:-${CRAWLER_DIR}/backups/${timestamp}}"
mkdir -p "${backup_dir}"

container_id="$(postgres_container_id)"

for db_name in "${DB_NAMES[@]}"; do
  dump_file="/tmp/${db_name}.dump"
  echo "Backing up ${db_name}"
  docker compose -f "${COMPOSE_FILE}" exec -T postgres pg_dump \
    --username postgres \
    --dbname "${db_name}" \
    --format custom \
    --file "${dump_file}"
  docker cp "${container_id}:${dump_file}" "${backup_dir}/${db_name}.dump"
  docker compose -f "${COMPOSE_FILE}" exec -T postgres rm -f "${dump_file}"
done

echo "Crawler Postgres backup written to ${backup_dir}"
