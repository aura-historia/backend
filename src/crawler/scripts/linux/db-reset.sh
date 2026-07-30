#!/usr/bin/env bash
set -euo pipefail

source "$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/db-common.sh"

ensure_local_crawler_compose
echo "Resetting local crawler dev Postgres only."

docker compose -f "${COMPOSE_FILE}" down -v
docker compose -f "${COMPOSE_FILE}" up -d

"$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/db-up.sh"
