#!/usr/bin/env bash
# Deploys one commit of this repository to the prod Compose project (host/prod/README.md#deploys).
# `host/install.sh deploy` installs this file as /usr/local/sbin/aura-prod-deploy; the deploy
# user's SSH key runs only that copy, as root through sudo. The installed copy runs, never the
# checkout's, so deploying an earlier commit rolls back without changing how deploys work.
#
# The request is SSH_ORIGINAL_COMMAND when set (the deploy user), otherwise the arguments:
#   deploy <commit> <image>  check out <commit> (40 hex digits) in the stable checkout, pull
#                            <image> (ghcr.io/aura-historia/aura-historia@sha256:<digest>, built
#                            from <commit>), apply its pending migrations, then restart api-a and
#                            api-b one at a time, the worker and mcp, and record the deployment
#   status                   print the deployed commit and the last deployments
set -euo pipefail

readonly CHECKOUT=@CHECKOUT@
readonly LIB_DIR=/usr/local/lib/aura-prod-deploy
readonly COMPOSE_ENV=/etc/aura-historia/prod/compose.env
readonly STATE_DIR=/var/lib/aura-historia/prod
readonly HISTORY="${STATE_DIR}/deploys.log"
readonly LOCK=/run/lock/aura-prod-deploy.lock
readonly IMAGE_PATTERN='^ghcr\.io/aura-historia/aura-historia@sha256:([0-9a-f]{64})$'
readonly IMAGE_SOURCE=https://github.com/aura-historia/backend
# Caddy probes each API replica every 5 s (caddy/Caddyfile). After a replica is ready, wait one
# more probe so Caddy routes to it again before the other replica restarts.
readonly CADDY_PROBE_SETTLE_SECONDS=7
readonly READY_TIMEOUT_SECONDS=180
readonly WORKER_SETTLE_SECONDS=20
# Unused application images older than this are removed after a deploy; GHCR keeps them all.
readonly IMAGE_RETENTION=720h

log() { printf '==> %s\n' "$*"; }
fail() { printf 'error: %s\n' "$*" >&2; exit 1; }
usage() { fail "usage: deploy <commit> <image> | status"; }

checkout_git() { git -C "${CHECKOUT}" "$@"; }
compose() { "${CHECKOUT}/host/prod/compose.sh" "$@"; }

# The commit to return the checkout to if the deploy fails before it changes anything running.
restore_commit=""
restore_checkout_on_failure() {
  local status=$?
  if ((status != 0)) && [[ -n ${restore_commit} ]]; then
    checkout_git checkout --quiet --detach "${restore_commit}" &&
      printf 'error: deploy failed before any service restarted; the checkout is back at %s\n' "${restore_commit}" >&2
  fi
  exit "${status}"
}

main() {
  [[ ${EUID} -eq 0 ]] || fail "run as root"
  local verb="" commit="" image="" extra=""
  read -r verb commit image extra <<<"${SSH_ORIGINAL_COMMAND-$*}" || true
  case ${verb} in
    deploy) [[ -n ${image} && -z ${extra} ]] || usage; deploy "${commit}" "${image}" ;;
    status) [[ -z ${commit}${image}${extra} ]] || usage; status ;;
    *) usage ;;
  esac
}

deploy() {
  local commit=$1 image=$2 digest previous
  [[ ${commit} =~ ^[0-9a-f]{40}$ ]] || fail "the commit must be 40 lowercase hex digits"
  [[ ${image} =~ ${IMAGE_PATTERN} ]] || fail "the image must be ghcr.io/aura-historia/aura-historia@sha256:<digest>"
  digest=${BASH_REMATCH[1]}
  exec 9>"${LOCK}"
  flock -n 9 || fail "another deploy is running"
  previous="$(checkout_git rev-parse HEAD)"
  require_caddy
  log "deploying ${commit} (the checkout was at ${previous})"

  trap restore_checkout_on_failure EXIT
  check_out "${commit}"
  restore_commit=${previous}
  pull_image "${image}" "${commit}"
  # Compose reads the image from the environment before compose.env, so the one-shot commands
  # below run the new image while the long-running services keep the old one.
  export AURA_HISTORIA_IMAGE=${image} AURA_HISTORIA_REVISION=${commit}
  migrate "${commit}" "${digest}"

  # From here the new release is the host's: restarts, timers and reboots use it.
  restore_commit=""
  record_release "${image}" "${commit}"
  restart_api
  restart_worker
  log "restarting mcp"
  recreate mcp >/dev/null
  wait_until_ready mcp mcp:8080/ready
  install -d -m 0750 "${STATE_DIR}"
  printf '%s %s %s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "${commit}" "${image}" "${previous}" >>"${HISTORY}"
  report_operator_changes "${previous}" "${commit}"
  docker image prune --all --force --filter "label=org.opencontainers.image.source=${IMAGE_SOURCE}" \
    --filter "until=${IMAGE_RETENTION}" >/dev/null || log "could not prune old images"
  log "deployed ${commit}"
}

status() {
  printf 'checkout %s\n' "$(checkout_git rev-parse HEAD)"
  grep -E '^AURA_HISTORIA_(IMAGE|REVISION)=' "${COMPOSE_ENV}" || true
  if [[ -s ${HISTORY} ]]; then
    echo "last deployments (time, commit, image, previous checkout):"
    tail -n 10 "${HISTORY}"
  fi
}

# Readiness is probed from Caddy's container, the way Caddy reaches the replicas.
require_caddy() {
  [[ -n "$(compose ps --quiet --status running caddy)" ]] || fail "Caddy is not running (host/prod/README.md#caddy)"
}

check_out() {
  [[ -z "$(checkout_git status --porcelain --untracked-files=no)" ]] || fail "${CHECKOUT} has local changes"
  checkout_git fetch --quiet origin "$1"
  checkout_git checkout --quiet --detach "$1"
}

pull_image() {
  local image=$1 commit=$2 revision
  docker pull --quiet "${image}" >/dev/null
  revision="$(docker image inspect --format '{{index .Config.Labels "org.opencontainers.image.revision"}}' "${image}")"
  [[ ${revision} == "${commit}" ]] || fail "${image} was built from ${revision:-an unknown commit}, not ${commit}"
}

# Applies the migrations the database lacks, as the migrator, with a plan checked against the
# inspected ledger (migration_plan.py). A rollback past applied migrations plans nothing: the
# schema only ever expands, and the older code runs on it.
migrate() {
  local commit=$1 digest=$2 work
  work="$(mktemp -d)"
  compose run --rm --no-deps -T ops inspect-schema >"${work}/inspection"
  python3 -I "${LIB_DIR}/migration_plan.py" --inspection "${work}/inspection" \
    --migrations "${CHECKOUT}/migrations" --bundle-digest "${digest}" \
    --request-id "deploy-${commit}-$(date -u +%Y%m%dT%H%M%SZ)" >"${work}/plan"
  if [[ -s ${work}/plan ]]; then
    log "applying migrations"
    compose run --rm --no-deps -T migrate migrate --plan /dev/stdin <"${work}/plan" | tail -n 1
  fi
  rm -rf -- "${work}"
}

record_release() {
  local image=$1 commit=$2 next
  next="$(mktemp "${COMPOSE_ENV}.XXXXXX")"
  awk -v image="${image}" -v revision="${commit}" '
    /^AURA_HISTORIA_IMAGE=/ { print "AURA_HISTORIA_IMAGE=" image; image_set = 1; next }
    /^AURA_HISTORIA_REVISION=/ { print "AURA_HISTORIA_REVISION=" revision; revision_set = 1; next }
    { print }
    END {
      if (!image_set) print "AURA_HISTORIA_IMAGE=" image
      if (!revision_set) print "AURA_HISTORIA_REVISION=" revision
    }' "${COMPOSE_ENV}" >"${next}"
  chown --reference="${COMPOSE_ENV}" "${next}"
  chmod --reference="${COMPOSE_ENV}" "${next}"
  mv -f -- "${next}" "${COMPOSE_ENV}"
}

container_of() { compose ps --quiet "$1"; }

probe() { compose exec -T caddy wget -q -O /dev/null "http://$1" 2>/dev/null; }

wait_until_ready() {
  local service=$1 url=$2 deadline=$((SECONDS + READY_TIMEOUT_SECONDS))
  until probe "${url}"; do
    ((SECONDS < deadline)) || fail "${service} is not ready ${READY_TIMEOUT_SECONDS} s after its restart"
    sleep 2
  done
}

# Recreates the service if its image or configuration changed; prints `recreated` or
# `unchanged`.
recreate() {
  local before
  before="$(container_of "$1")"
  compose up --detach --no-deps "$1" >&2
  if [[ "$(container_of "$1")" != "${before}" ]]; then echo recreated; else echo unchanged; fi
}

# One replica at a time: while one restarts, Caddy retries its requests on the other.
restart_api() {
  local replica outcome
  for replica in api-a api-b; do
    log "restarting ${replica}"
    outcome="$(recreate "${replica}")"
    wait_until_ready "${replica}" "${replica}:8080/v1/ready"
    [[ ${outcome} == unchanged ]] || sleep "${CADDY_PROBE_SETTLE_SECONDS}"
  done
}

# The worker finishes its in-flight jobs on SIGTERM (stop_grace_period) and serves no HTTP, so
# it passes when it is still running, without a restart, a while after starting.
restart_worker() {
  local state
  log "restarting the worker"
  [[ "$(recreate worker)" == recreated ]] || return 0
  sleep "${WORKER_SETTLE_SECONDS}"
  state="$(docker inspect --format '{{.State.Status}} {{.RestartCount}}' "$(container_of worker)")"
  [[ ${state} == "running 0" ]] || fail "the worker is '${state}' ${WORKER_SETTLE_SECONDS} s after its restart"
}

# The deploy restarts only the application services; these files take an operator's step.
report_operator_changes() {
  local changed
  changed="$(checkout_git diff --name-only "$1" "$2" -- host/prod/postgres host/prod/caddy \
    host/prod/systemd host/prod/tmpfiles host/prod/deploy.sh host/prod/migration_plan.py \
    host/deploy host/install.sh 2>/dev/null || true)"
  [[ -n ${changed} ]] || return 0
  log "changed since the last deploy, applied only by an operator (host/prod/README.md#deploys):"
  sed 's/^/    /' <<<"${changed}"
}

main "$@"
