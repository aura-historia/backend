#!/usr/bin/env bash
# Restores the newest backup and every archived WAL segment after it from the pgBackRest
# repository into this throwaway container, starts the copy without archiving, checks it and
# stops it. Runs as the `restore-check` Compose service (host/prod/README.md#restore).
set -euo pipefail

readonly DATA=/var/lib/postgresql/restore
readonly SOCKET=/tmp/restore-socket

if [[ ${EUID} -eq 0 ]]; then
  install -d -o postgres -g postgres -m 0700 "${DATA}" "${SOCKET}"
  exec gosu postgres "$0" "$@"
fi

log() { printf '==> %s\n' "$*"; }

[[ -z "$(ls -A "${DATA}")" ]] || { echo "error: ${DATA} is not empty" >&2; exit 1; }

log "restoring the newest backup and the WAL archive"
pgbackrest --stanza=aura --pg1-path="${DATA}" --archive-mode=off restore
pgbackrest --stanza=aura info

# archive_mode=off keeps the copy from pushing WAL into the production repository.
log "replaying WAL into the copy"
pg_ctl -D "${DATA}" -w -t 3600 start -o "-c config_file=/etc/postgresql/postgresql.conf \
  -c data_directory=${DATA} -c archive_mode=off -c listen_addresses='' \
  -c unix_socket_directories=${SOCKET} -c shared_buffers=512MB -c huge_pages=off \
  -c effective_cache_size=1GB -c maintenance_work_mem=256MB"
until [[ "$(psql -h "${SOCKET}" -d aura -XAtc 'SELECT NOT pg_is_in_recovery()')" == t ]]; do
  sleep 2
done

psql -h "${SOCKET}" -d aura -X <<'SQL'
SELECT pg_last_xact_replay_timestamp() AS last_replayed_transaction,
       (SELECT count(*) FROM public._sqlx_migrations WHERE success) AS applied_migrations,
       (SELECT count(*) FROM pg_catalog.pg_tables WHERE schemaname = 'public') AS public_tables;
SQL

pg_ctl -D "${DATA}" -w -m fast stop
log "verifying data checksums of the stopped copy"
pg_checksums --check -D "${DATA}"
log "restore check passed"
