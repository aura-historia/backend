#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
CRAWLER_DIR="$(cd -- "${SCRIPT_DIR}/../.." && pwd)"
export MIGRATIONS_DIR="${CRAWLER_DIR}/migrations"
export COMPOSE_FILE="${CRAWLER_DIR}/docker-compose.yml"

DB_URLS=(
  "postgres://postgres:postgres@localhost:5432/crawler_server"
  "postgres://postgres:postgres@localhost:5432/crawler_demo"
  "postgres://postgres:postgres@localhost:5432/crawler_demo_scraper"
  "postgres://postgres:postgres@localhost:5432/crawler_demo_spider"
)
export DB_URLS

DB_NAMES=(
  "crawler_server"
  "crawler_demo"
  "crawler_demo_scraper"
  "crawler_demo_spider"
)
export DB_NAMES

ensure_local_crawler_compose() {
  if [[ ! -f "${COMPOSE_FILE}" ]]; then
    echo "Crawler Docker Compose file not found: ${COMPOSE_FILE}" >&2
    exit 1
  fi

  if ! docker compose -f "${COMPOSE_FILE}" config --services | grep -Fxq "postgres"; then
    echo "Crawler Docker Compose file has no postgres service: ${COMPOSE_FILE}" >&2
    exit 1
  fi
}

postgres_container_id() {
  local container_id
  container_id="$(docker compose -f "${COMPOSE_FILE}" ps -q postgres)"
  if [[ -z "${container_id}" ]]; then
    echo "Local crawler Postgres container is not running. Run db-up first." >&2
    exit 1
  fi

  echo "${container_id}"
}
