#!/usr/bin/env bash
set -euo pipefail

source "$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/db-common.sh"

backup_retention_days="${BACKUP_RETENTION_DAYS:-14}"
backup_root_dir="${CRAWLER_DIR}/backups"

default_backup_dir="${backup_root_dir}/$(date -u +"%Y%m%dT%H%M%SZ")"
backup_dir="${1:-${default_backup_dir}}"

ensure_local_crawler_compose
docker compose -f "${COMPOSE_FILE}" up -d

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

if [[ "${backup_dir}" == "${backup_root_dir}"/* && "${backup_retention_days}" =~ ^[0-9]+$ && "${backup_retention_days}" -gt 0 ]]; then
  echo "Pruning backups older than ${backup_retention_days} day(s) from ${backup_root_dir}"
  find "${backup_root_dir}" -mindepth 1 -maxdepth 1 -type d -mtime +"${backup_retention_days}" -exec rm -rf -- {} +
fi

echo "Crawler Postgres backup written to ${backup_dir}"
