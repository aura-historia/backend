#!/usr/bin/env bash
set -euo pipefail

source "$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/db-common.sh"

docker compose -f "${COMPOSE_FILE}" down -v
docker compose -f "${COMPOSE_FILE}" up -d

"$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/db-up.sh"
