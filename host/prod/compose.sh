#!/usr/bin/env bash
# Runs `docker compose` for the prod project with the host's settings. Run as root: Compose
# reads the root-only secret files. Example: sudo host/prod/compose.sh ps
set -euo pipefail

PROD_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly PROD_DIR
readonly CONFIG_DIR=/etc/aura-historia/prod

[[ ${EUID} -eq 0 ]] || { printf 'error: run as root (sudo %s)\n' "$0" >&2; exit 1; }
exec docker compose --project-directory "${PROD_DIR}" -f "${PROD_DIR}/compose.yaml" \
  --env-file "${CONFIG_DIR}/compose.env" "$@"
