#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
CRAWLER_DIR="$(cd -- "${SCRIPT_DIR}/.." && pwd)"
export MIGRATIONS_DIR="${CRAWLER_DIR}/migrations"
export COMPOSE_FILE="${CRAWLER_DIR}/docker-compose.yml"

DB_URLS=(
  "postgres://postgres:postgres@localhost:5432/crawler_server"
  "postgres://postgres:postgres@localhost:5432/crawler_demo"
  "postgres://postgres:postgres@localhost:5432/crawler_demo_scraper"
  "postgres://postgres:postgres@localhost:5432/crawler_demo_spider"
)
export DB_URLS
