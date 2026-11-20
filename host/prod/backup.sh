#!/usr/bin/env bash
# pgBackRest operations for the prod database (host/prod/README.md#backups). Run as root.
#
# Usage: sudo host/prod/backup.sh <command>
#   stanza         create the stanza (idempotent) and check that WAL archiving reaches S3
#   full | diff    take a full or differential backup, then expire past retention; record
#                  its success for the health metrics
#   info           print the repository's backups and WAL range
#   restore-check  restore the newest backup and the WAL archive into a throwaway container
#                  and check it; never touches the running database
set -euo pipefail

PROD_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly PROD_DIR

# The worker's health metrics report the time since this file last changed
# (src/aura-historia-worker/README.md#health-metrics).
readonly BACKUP_MARKER=/var/lib/aura-historia/prod/backup/last-success

compose() { "${PROD_DIR}/compose.sh" "$@"; }

# Backups run inside the database container at a lower CPU priority than the database.
pgbackrest() { compose exec -T -u postgres postgres nice -n 10 pgbackrest --stanza=aura "$@"; }

main() {
  case ${1:-} in
    stanza)
      pgbackrest stanza-create
      pgbackrest check
      ;;
    full | diff)
      pgbackrest --type="$1" backup
      install -d -m 0755 "${BACKUP_MARKER%/*}"
      touch "${BACKUP_MARKER}"
      ;;
    info) pgbackrest info ;;
    restore-check) compose run --rm --no-deps -T restore-check ;;
    *) sed -n '2,11p' "$0"; exit 2 ;;
  esac
}

main "$@"
