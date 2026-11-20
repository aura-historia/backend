#!/usr/bin/env bash
# Smoke test of the prod Compose project (host/prod) with throwaway secrets. It runs the
# checked-in compose file under its own project, volume and network names:
#   - PostgreSQL starts with data checksums, and only the expected ports and sockets;
#   - `bootstrap`, `inspect-schema` and `inspect` run through the ops entrypoint, and the
#     migrator accepts the migration plan the deploy computes (host/prod/migration_plan.py);
#   - pgBackRest creates the stanza, archives WAL, takes a full and a differential backup, and
#     `restore-check` restores the newest one plus the archive into a throwaway container. The
#     encrypted repository is a local volume here; the host's first backup proves S3;
#   - Caddy refuses requests without the origin secret, forwards those with it and never logs it.
# Application processes that need real provider credentials at startup are not started.
#
# Usage: ci/host-prod/smoke.sh <application image>
set -euo pipefail

readonly APP_IMAGE="${1:?usage: smoke.sh <application image>}"
REPO="$(git rev-parse --show-toplevel)"
readonly REPO PROD_DIR="${REPO}/host/prod"
readonly PROJECT=aura-prod-smoke
readonly HTTPS_PORT=18443
WORK="$(mktemp -d)"
readonly WORK CONFIG="${WORK}/config"
readonly ORIGIN_SECRET=smoke-origin-secret

log() { printf '==> %s\n' "$*"; }
fail() { printf 'smoke: %s\n' "$*" >&2; exit 1; }

compose() {
  docker compose -p "${PROJECT}" --project-directory "${PROD_DIR}" \
    -f "${WORK}/compose.yaml" -f "${WORK}/smoke.yaml" --env-file "${CONFIG}/compose.env" "$@"
}

cleanup() {
  local status=$?
  if [[ ${status} -ne 0 ]]; then
    compose ps -a || true
    compose logs --no-color --tail 80 || true
  fi
  compose --profile '*' down -v --remove-orphans >/dev/null 2>&1 || true
  docker volume rm aura-smoke-postgres-data aura-smoke-caddy-data >/dev/null 2>&1 || true
  docker network rm aura-smoke-search >/dev/null 2>&1 || true
  rm -rf -- "${WORK}"
  exit "${status}"
}
trap cleanup EXIT

# Every template value: generated groups get one random value each, everything else a dummy.
write_config() {
  local relative group
  declare -A generated=()
  mkdir -p "${CONFIG}/secrets" "${WORK}/socket" "${WORK}/backup"
  chmod 0777 "${WORK}/socket"
  while read -r relative; do
    sed -E 's/^([A-Z0-9_]+)=$/\1=smoke-\1/' "${PROD_DIR}/templates/${relative}" >"${CONFIG}/${relative}"
    for group in $(grep -oE '@generate:[a-z-]+@' "${CONFIG}/${relative}" | sort -u); do
      [[ -n ${generated[${group}]:-} ]] || generated[${group}]="$(openssl rand -hex 16)"
      sed -i "s/${group}/${generated[${group}]}/g" "${CONFIG}/${relative}"
    done
  done < <(cd "${PROD_DIR}/templates" && find . -name '*.env' -printf '%P\n')
  cat >"${CONFIG}/compose.env" <<EOF
AURA_HISTORIA_IMAGE=${APP_IMAGE}
AURA_HISTORIA_REVISION=smoke
AURA_BACKUP_BUCKET=aura-smoke-backups
EOF
  cat >"${CONFIG}/secrets/caddy.env" <<EOF
CLOUDFLARE_DNS_API_TOKEN=unused
ORIGIN_VERIFY=${ORIGIN_SECRET}
EOF
  cat >>"${CONFIG}/secrets/pgbackrest.env" <<'EOF'
PGBACKREST_REPO1_TYPE=posix
PGBACKREST_REPO1_PATH=/var/lib/pgbackrest
EOF
  echo '{"type": "external_account"}' >"${CONFIG}/google-credentials.json"
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 1 -subj /CN=smoke \
    -keyout /dev/null -out "${CONFIG}/opensearch-root-ca.pem" 2>/dev/null
}

# The checked-in project under smoke names, without host placement or CloudWatch logs, with a
# local CA instead of the DNS challenge and HTTPS on a loopback port.
write_compose() {
  sed -e "s|/etc/aura-historia/prod|${CONFIG}|g" \
      -e "s|/run/aura-historia/postgresql|${WORK}/socket|g" \
      -e "s|/var/lib/aura-historia/prod/backup|${WORK}/backup|g" \
      -e 's|driver: awslogs|driver: local|' -e '/options: {<<: \*log-options/d' \
      -e 's|name: aura-historia-prod-|name: aura-smoke-|' \
      -e 's|name: aura-prod-|name: aura-smoke-|' \
      -e 's|name: aura-opensearch-prod_search|name: aura-smoke-search|' \
      -e "s|- 443:443\$|- 127.0.0.1:${HTTPS_PORT}:443|" -e '/- 443:443\/udp/d' \
      -e '/cgroup_parent:/d' \
      -e "s|./caddy/Caddyfile:|${WORK}/Caddyfile:|" \
      "${PROD_DIR}/compose.yaml" >"${WORK}/compose.yaml"
  sed -e 's|acme_dns cloudflare .*|local_certs|' "${PROD_DIR}/caddy/Caddyfile" >"${WORK}/Caddyfile"
  cat >"${WORK}/smoke.yaml" <<'EOF'
services:
  postgres:
    volumes: [repository:/var/lib/pgbackrest]
  restore-check:
    volumes: [repository:/var/lib/pgbackrest]
volumes:
  repository:
EOF
}

psql_aura() { compose exec -T -u postgres postgres psql -d aura -XAtc "$1"; }
psql_superuser_over_tcp() { compose exec -T -u postgres postgres psql -h 127.0.0.1 -d aura -XAtc 'SELECT 1'; }
pgbackrest() { compose exec -T -u postgres postgres pgbackrest --stanza=aura "$@"; }
plan_migrations() {
  python3 -I "${PROD_DIR}/migration_plan.py" --inspection "$1" --migrations "${REPO}/migrations" \
    --request-id smoke --bundle-digest "$(printf '%064d' 0)"
}

# The deploy's plan against the migrator: nothing is pending after bootstrap, and a plan made
# before the newest migration was applied is accepted as already committed, which proves the
# plan's checksums and ledger fingerprint are the migrator's own.
check_migration_plan() {
  log "checking the deploy's migration plan against the migrator"
  compose --profile ops run --rm -T ops inspect-schema >"${WORK}/inspection"
  plan_migrations "${WORK}/inspection" >"${WORK}/plan"
  [[ ! -s ${WORK}/plan ]] || fail "a bootstrapped database has pending migrations"
  python3 -I - "${WORK}/inspection" "${PROD_DIR}" >"${WORK}/earlier-inspection" <<'PYTHON'
import json, sys
sys.path.insert(0, sys.argv[2])
import migration_plan
inspection = migration_plan.read_inspection(sys.argv[1])
inspection["ledger"] = inspection["ledger"][:-1]
inspection["ledger_fingerprint"] = migration_plan.fingerprint(inspection["ledger"])
print(json.dumps(inspection))
PYTHON
  plan_migrations "${WORK}/earlier-inspection" >"${WORK}/plan"
  [[ -s ${WORK}/plan ]] || fail "no plan for the newest migration"
  compose --profile ops run --rm -T migrate migrate --plan /dev/stdin <"${WORK}/plan" | tail -n 1 >"${WORK}/migrate.json"
  grep -q '"status":"NO_OP"' "${WORK}/migrate.json" || fail "the migrator refused the deploy's plan: $(cat "${WORK}/migrate.json")"
}

status_with_origin_header() {
  local header=()
  [[ -z $1 ]] || header=(-H "X-Origin-Verify: $1")
  curl -sk -o /dev/null -w '%{http_code}' --resolve "api.aura-historia.com:${HTTPS_PORT}:127.0.0.1" \
    "${header[@]}" "https://api.aura-historia.com:${HTTPS_PORT}/v1/health"
}

main() {
  write_config
  write_compose
  docker network create aura-smoke-search >/dev/null
  docker volume create aura-smoke-postgres-data >/dev/null
  docker volume create aura-smoke-caddy-data >/dev/null
  compose config --quiet

  log "building the database and proxy images"
  compose build postgres caddy

  log "starting PostgreSQL"
  compose up -d --wait postgres
  [[ "$(psql_aura 'SHOW data_checksums')" == on ]] || fail "data checksums are off"
  [[ -S ${WORK}/socket/.s.PGSQL.5432 ]] || fail "no PostgreSQL socket in the shared directory"
  [[ -z "$(docker port "$(compose ps -q postgres)")" ]] || fail "PostgreSQL publishes a port"
  # pgBackRest verifies S3's certificate against the system CA bundle.
  compose exec -T postgres test -s /etc/ssl/certs/ca-certificates.crt || fail "the database image has no CA bundle"

  log "bootstrapping roles and migrations"
  compose run --rm -T bootstrap | tee "${WORK}/bootstrap.json"
  grep -q '"BOOTSTRAPPED"' "${WORK}/bootstrap.json" || fail "bootstrap did not report BOOTSTRAPPED"
  compose --profile ops run --rm -T ops inspect >/dev/null
  check_migration_plan
  if psql_superuser_over_tcp >/dev/null 2>&1; then
    fail "the superuser connected over the network"
  fi

  log "archiving WAL and backing up"
  pgbackrest stanza-create
  pgbackrest check
  pgbackrest --type=full backup
  psql_aura "CREATE TABLE public.smoke_marker AS SELECT 'after-full'::text AS value" >/dev/null
  pgbackrest --type=diff backup
  psql_aura "INSERT INTO public.smoke_marker VALUES ('after-diff')" >/dev/null
  psql_aura "SELECT pg_switch_wal()" >/dev/null
  pgbackrest check

  log "restoring into a throwaway container"
  compose --profile backup run --rm -T restore-check | tee "${WORK}/restore.log"
  grep -q 'restore check passed' "${WORK}/restore.log" || fail "restore check did not pass"

  log "checking the origin lock"
  compose up -d --no-deps caddy
  local status
  for _ in $(seq 30); do
    status="$(status_with_origin_header "" || true)"
    [[ ${status} == 403 ]] && break
    sleep 1
  done
  [[ ${status} == 403 ]] || fail "a request without the origin secret got ${status}, not 403"
  [[ "$(status_with_origin_header wrong-secret)" == 403 ]] || fail "a wrong origin secret was accepted"
  status="$(status_with_origin_header "${ORIGIN_SECRET}")"
  # No API replica runs here, so a forwarded request ends at the proxy as 502 or 503.
  [[ ${status} == 502 || ${status} == 503 ]] || fail "the origin secret was not forwarded (${status})"
  if compose logs --no-color caddy 2>&1 | grep -qF -- "${ORIGIN_SECRET}"; then
    fail "Caddy logged the origin secret"
  fi
  log "smoke test passed"
}

main
