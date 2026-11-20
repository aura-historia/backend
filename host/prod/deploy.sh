#!/usr/bin/env bash
# Deploys one commit of this repository to the prod Compose project (host/prod/README.md#deploys).
# `host/install.sh deploy` installs this file as /usr/local/sbin/aura-prod-deploy; the deploy
# user's SSH key runs only that copy, as root through sudo. The installed copy runs, never the
# checkout's, so deploying an earlier commit rolls back without changing how deploys work.
#
# Requests, from SSH_ORIGINAL_COMMAND for the deploy user, otherwise from the arguments:
#   deploy <commit> <image>  run the deploy as the transient unit aura-prod-deploy.service and
#                            stream its log. The unit outlives a dropped SSH session; a second
#                            deploy is refused while one runs.
#                            <commit>: 40 hex digits, on one of DEPLOY_BRANCHES
#                            <image>:  ghcr.io/aura-historia/aura-historia@sha256:<digest>, built
#                                      from <commit>
#   status                   the deployed commit, the last deployments and the last deploy's log
#   run <commit> <image>     the deploy itself, as the unit runs it; not accepted over SSH
set -euo pipefail
shopt -s inherit_errexit

readonly SELF=/usr/local/sbin/aura-prod-deploy
readonly UNIT=aura-prod-deploy.service
readonly CHECKOUT=@CHECKOUT@
readonly LIB_DIR=/usr/local/lib/aura-prod-deploy
readonly COMPOSE_ENV=/etc/aura-historia/prod/compose.env
readonly STATE_DIR=/var/lib/aura-historia/prod
readonly HISTORY="${STATE_DIR}/deploys.log"
readonly LOCK=/run/aura-prod-deploy.lock
readonly IMAGE_PATTERN='^ghcr\.io/aura-historia/aura-historia@sha256:([0-9a-f]{64})$'
readonly IMAGE_SOURCE=https://github.com/aura-historia/backend
# Only commits on these branches deploy; a commit elsewhere, such as one from a fork, never
# reaches the checkout the host's timers run as root. Keep in sync with deploy.yml.
readonly DEPLOY_BRANCHES=(develop)
readonly APPLICATION_SERVICES=(api-a api-b worker mcp)
# Caddy probes each API replica every 5 s and, after a failed request, leaves it out for 30 s
# (caddy/Caddyfile). Before the other replica restarts, the restarted one must be back in.
readonly CADDY_ADMIN=http://127.0.0.1:2019
readonly CADDY_PROBE_SETTLE_SECONDS=7
readonly READY_TIMEOUT_SECONDS=180
readonly ROUTED_TIMEOUT_SECONDS=90
readonly WORKER_SETTLE_SECONDS=20
readonly DEPLOY_TIMEOUT=1h
# Unused application images built longer ago than this are removed; GHCR keeps every image.
readonly IMAGE_RETENTION=720h

log() { printf '==> %s\n' "$*"; }
fail() { printf 'error: %s\n' "$*" >&2; exit 1; }
usage() { fail "usage: deploy <commit> <image> | status"; }

checkout_git() { git -C "${CHECKOUT}" "$@"; }
compose() { "${CHECKOUT}/host/prod/compose.sh" "$@"; }

main() {
  [[ ${EUID} -eq 0 ]] || fail "run as root"
  local request verbs verb="" commit="" image="" extra=""
  if [[ -n ${SSH_ORIGINAL_COMMAND+set} ]]; then
    request=${SSH_ORIGINAL_COMMAND}
    verbs=" deploy status "
  else
    request="$*"
    verbs=" deploy status run "
  fi
  read -r verb commit image extra <<<"${request}" || true
  [[ -n ${verb} && ${verbs} == *" ${verb} "* ]] || usage
  case ${verb} in
    deploy | run)
      [[ -n ${image} && -z ${extra} ]] || usage
      [[ ${commit} =~ ^[0-9a-f]{40}$ ]] || fail "the commit must be 40 lowercase hex digits"
      [[ ${image} =~ ${IMAGE_PATTERN} ]] || fail "the image must be ghcr.io/aura-historia/aura-historia@sha256:<digest>"
      if [[ ${verb} == deploy ]]; then start "${commit}" "${image}"; else run "${commit}" "${image}"; fi
      ;;
    status)
      [[ -z ${commit}${image}${extra} ]] || usage
      status
      ;;
  esac
}

# Runs the deploy as a transient unit and follows its log until it ends.
start() {
  local commit=$1 image=$2 since follower result=0
  ! systemctl is-active --quiet "${UNIT}" || fail "another deploy is running (journalctl -u ${UNIT})"
  since="$(date '+%Y-%m-%d %H:%M:%S')"
  journalctl --follow --quiet --output=cat --unit="${UNIT}" --since="${since}" &
  follower=$!
  systemd-run --quiet --wait --collect --unit="${UNIT}" --description="Aura Historia prod deploy of ${commit}" \
    --property=RuntimeMaxSec="${DEPLOY_TIMEOUT}" --setenv=HOME=/root "${SELF}" run "${commit}" "${image}" || result=$?
  sleep 2
  kill "${follower}" 2>/dev/null || true
  ((result == 0)) || fail "the deploy failed (status ${result}); 'status' or journalctl -u ${UNIT} shows its log"
}

status() {
  printf 'checkout %s\n' "$(checkout_git rev-parse HEAD)"
  grep -E '^AURA_HISTORIA_(IMAGE|REVISION)=' "${COMPOSE_ENV}" || true
  if systemctl is-active --quiet "${UNIT}"; then echo "a deploy is running"; fi
  if [[ -s ${HISTORY} ]]; then
    echo "last deployments (time, commit, image, previous checkout):"
    tail -n 10 "${HISTORY}"
  fi
  echo "last deploy's log:"
  journalctl --unit="${UNIT}" --lines=40 --output=short-iso --no-pager --quiet || true
}

# The commit to return the checkout to if the deploy fails before it changes anything running.
restore_commit=""
restore_checkout_on_failure() {
  local status=$?
  if ((status != 0)) && [[ -n ${restore_commit} ]]; then
    checkout_git checkout --quiet --detach "${restore_commit}" &&
      printf 'error: no service was restarted; the checkout is back at %s\n' "${restore_commit}" >&2
  fi
  exit "${status}"
}

run() {
  local commit=$1 image=$2 digest previous
  [[ ${image} =~ ${IMAGE_PATTERN} ]]
  digest=${BASH_REMATCH[1]}
  exec 9>"${LOCK}"
  flock -n 9 || fail "another deploy is running"
  previous="$(checkout_git rev-parse HEAD)"
  require_caddy
  log "deploying ${commit} (the checkout was at ${previous})"

  require_deployable "${commit}"
  pull_image "${image}" "${commit}"
  trap restore_checkout_on_failure EXIT
  restore_commit=${previous}
  check_out "${commit}"
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
  require_running_image "${image}"
  install -d -m 0750 "${STATE_DIR}"
  printf '%s %s %s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "${commit}" "${image}" "${previous}" >>"${HISTORY}"
  report_operator_changes "${previous}" "${commit}"
  docker image prune --all --force --filter "label=org.opencontainers.image.source=${IMAGE_SOURCE}" \
    --filter "until=${IMAGE_RETENTION}" >/dev/null || log "could not prune old images"
  log "deployed ${commit}"
}

# Readiness is probed from Caddy's container, the way Caddy reaches the replicas.
require_caddy() {
  [[ -n "$(compose ps --quiet --status running caddy)" ]] || fail "Caddy is not running (host/prod/README.md#caddy)"
}

# Fetches only the deploy branches, so a commit outside them is never even fetched.
require_deployable() {
  local commit=$1 branch
  local refspecs=()
  for branch in "${DEPLOY_BRANCHES[@]}"; do refspecs+=("+refs/heads/${branch}:refs/remotes/origin/${branch}"); done
  checkout_git fetch --quiet origin "${refspecs[@]}"
  if checkout_git cat-file -e "${commit}^{commit}" 2>/dev/null; then
    for branch in "${DEPLOY_BRANCHES[@]}"; do
      checkout_git merge-base --is-ancestor "${commit}" "refs/remotes/origin/${branch}" && return 0
    done
  fi
  fail "${commit} is not on ${DEPLOY_BRANCHES[*]}"
}

pull_image() {
  local image=$1 commit=$2 revision
  docker pull --quiet "${image}" >/dev/null
  revision="$(docker image inspect --format '{{index .Config.Labels "org.opencontainers.image.revision"}}' "${image}")"
  [[ ${revision} == "${commit}" ]] || fail "${image} was built from ${revision:-an unknown commit}, not ${commit}"
}

check_out() {
  [[ -z "$(checkout_git status --porcelain --untracked-files=no)" ]] || fail "${CHECKOUT} has local changes"
  checkout_git checkout --quiet --detach "$1"
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
    compose run --rm --no-deps -T migrate migrate --plan /dev/stdin <"${work}/plan" ||
      fail "the migrator failed; migrations it committed stay applied (ops inspect-schema shows the ledger)"
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

# Caddy's failure count for one upstream, from its admin API.
caddy_failures() {
  compose exec -T caddy wget -q -O - "${CADDY_ADMIN}/reverse_proxy/upstreams" |
    python3 -I -c 'import json, sys
print(next(u["fails"] for u in json.load(sys.stdin) if u["address"] == sys.argv[1]))' "$1"
}

# Waits until Caddy no longer holds a failed request against the replica, then one more active
# probe, so Caddy routes to it again.
wait_until_routed() {
  local replica=$1 deadline=$((SECONDS + ROUTED_TIMEOUT_SECONDS))
  until [[ "$(caddy_failures "${replica}:8080")" == 0 ]]; do
    ((SECONDS < deadline)) || fail "Caddy still counts failures against ${replica} ${ROUTED_TIMEOUT_SECONDS} s after it became ready"
    sleep 2
  done
  sleep "${CADDY_PROBE_SETTLE_SECONDS}"
}

# Recreates the service if its image or configuration changed; prints `recreated` or
# `unchanged`.
recreate() {
  local before
  before="$(container_of "$1")"
  compose up --detach --no-deps "$1" >&2 || fail "compose could not start $1"
  if [[ "$(container_of "$1")" != "${before}" ]]; then echo recreated; else echo unchanged; fi
}

# One replica at a time: while one restarts, Caddy retries its requests on the other.
restart_api() {
  local replica outcome
  for replica in api-a api-b; do
    log "restarting ${replica}"
    outcome="$(recreate "${replica}")"
    wait_until_ready "${replica}" "${replica}:8080/v1/ready"
    [[ ${outcome} == unchanged ]] || wait_until_routed "${replica}"
  done
}

# The worker finishes its in-flight jobs on SIGTERM (stop_grace_period) and serves no HTTP, so
# it passes when it is still running, without a restart, a while after starting.
restart_worker() {
  local outcome state
  log "restarting the worker"
  outcome="$(recreate worker)"
  [[ ${outcome} == recreated ]] || return 0
  sleep "${WORKER_SETTLE_SECONDS}"
  state="$(docker inspect --format '{{.State.Status}} {{.RestartCount}}' "$(container_of worker)")"
  [[ ${state} == "running 0" ]] || fail "the worker is '${state}' ${WORKER_SETTLE_SECONDS} s after its restart"
}

require_running_image() {
  local expected service actual
  expected="$(docker image inspect --format '{{.Id}}' "$1")"
  for service in "${APPLICATION_SERVICES[@]}"; do
    actual="$(docker inspect --format '{{.Image}}' "$(container_of "${service}")")"
    [[ ${actual} == "${expected}" ]] || fail "${service} does not run the deployed image"
  done
}

# The deploy restarts only the application services; these files take an operator's step.
report_operator_changes() {
  local changed
  changed="$(checkout_git diff --name-only "$1" "$2" -- host/prod/postgres host/prod/caddy \
    host/prod/systemd host/prod/tmpfiles host/prod/docker host/prod/deploy.sh \
    host/prod/migration_plan.py host/deploy host/install.sh 2>/dev/null || true)"
  [[ -n ${changed} ]] || return 0
  log "changed since the last deploy, applied only by an operator (host/prod/README.md#deploys):"
  sed 's/^/    /' <<<"${changed}"
}

main "$@"
