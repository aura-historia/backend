#!/usr/bin/env bash
# Deploys a released crawler build on this host, or rolls back to an earlier one: run it with the
# commit SHA of a published crawler-<sha> GitHub Release, or with the name of a kept legacy-<time>
# build. Pull-based: the host downloads the release, so CI holds no host credentials, and needs
# neither Git, Cargo nor a source tree.
# A release that adds crawler-local migrations is not backed up here: the weekly S3 backup
# (crawler-backup.timer) is the only backup. Before such a deploy run it by hand:
# systemctl start crawler-backup.service. A failure after the services were stopped starts them again.
# --unattended is for the autodeploy timer: it refuses (exit 3) a release that adds migrations or a
# rollback over applied migrations, which need an operator, and rolls an unhealthy release back
# (exit 4) to the build it replaced.
# Usage: deploy.sh [--install-units] [--db-restored] [--unattended] <full lowercase sha | legacy-<time>>
# Contract: docs/crawler/operations.md#deploy-and-roll-back
set -euo pipefail

ROOT="${CRAWLER_ROOT:-/opt/crawler}"
REPOSITORY="${CRAWLER_RELEASE_REPOSITORY:-aura-historia/backend}"
BASE_URL="${CRAWLER_RELEASE_BASE_URL:-https://github.com/${REPOSITORY}/releases/download}"
HEALTH_URL="${CRAWLER_HEALTH_URL:-http://127.0.0.1:7878/health}"
# Seconds a started build has to report healthy; startup applies pending migrations first.
HEALTH_TIMEOUT="${CRAWLER_HEALTH_TIMEOUT:-600}"
HEALTH_INTERVAL="${CRAWLER_HEALTH_INTERVAL:-5}"
KEEP="${CRAWLER_KEEP_RELEASES:-5}"
UNIT_DIR="${CRAWLER_UNIT_DIR:-/etc/systemd/system}"
readonly SERVER_UNIT=crawler-server
readonly RUNNER_UNIT=review-agent-runner
readonly EXIT_NEEDS_OPERATOR=3
readonly EXIT_UNHEALTHY=4
readonly UNITS=("${SERVER_UNIT}.service" "${RUNNER_UNIT}.service")
# Present in a release; managed only on a host that installed them.
readonly OPTIONAL_UNITS=(crawler-autodeploy.service crawler-autodeploy.timer crawler-backup.service crawler-backup.timer)
# Written last into an unpacked release: a directory without it is never trusted.
readonly COMPLETE_MARKER=.complete
readonly USAGE="usage: ${0##*/} [--install-units] [--db-restored] [--unattended] <full lowercase sha | legacy-<time>>"

install_units=false
db_restored=false
unattended=false
build=""
for arg in "$@"; do
  case "$arg" in
    --install-units) install_units=true ;;
    --db-restored) db_restored=true ;;
    --unattended) unattended=true ;;
    -*) echo "unknown option: $arg" >&2; exit 2 ;;
    *) [[ -z "$build" ]] || { echo "give exactly one build" >&2; exit 2; }; build="$arg" ;;
  esac
done
legacy=false
if [[ "$build" =~ ^legacy-[0-9]{8}T[0-9]{6}Z$ ]]; then
  legacy=true
elif [[ ! "$build" =~ ^[a-f0-9]{40}$ ]]; then
  echo "$USAGE" >&2
  exit 2
fi
for number in "$HEALTH_TIMEOUT" "$HEALTH_INTERVAL" "$KEEP"; do
  [[ "$number" =~ ^[0-9]+$ ]] || { echo "health and retention settings must be whole numbers" >&2; exit 2; }
done

releases="${ROOT}/releases"
target="${releases}/${build}"
link="${ROOT}/bin"
name="crawler-${build}"
work=""
services_stopped=false
runner_was_active=false
# The build to switch back to if the deploy fails after the switch and before the new build started:
# such a build was never health-checked, so it must not be left running.
switched_from=""

start_services() {
  systemctl start "$SERVER_UNIT"
  ! $runner_was_active || systemctl start "$RUNNER_UNIT"
  services_stopped=false
}

on_exit() {
  local status=$?
  if $services_stopped; then
    if [[ -n "$switched_from" ]]; then
      echo "switching back to $(basename "$switched_from")" >&2
      switch_release "$switched_from" || echo "could not switch back to ${switched_from}" >&2
    fi
    echo "the deploy failed with the services stopped; starting $(basename "$(readlink -f "$link")")" >&2
    start_services || echo "could not start the crawler's units; see: systemctl status ${SERVER_UNIT}" >&2
    # Not exit 1, which autodeploy retries every run: each retry would stop the services again.
    # Exit 4 leaves the release to an operator, like an unhealthy one.
    ((status != 1)) || status=$EXIT_UNHEALTHY
  fi
  [[ -z "$work" ]] || rm -rf -- "$work"
  exit "$status"
}
trap on_exit EXIT
trap 'exit 130' INT
trap 'exit 143' TERM HUP

# One deploy at a time, whether started by hand or by the timer.
mkdir -p "$releases"
exec 9> "${ROOT}/deploy.lock"
flock -n 9 || { echo "another deploy is running" >&2; exit 1; }
# Left by a deploy that was killed; no other deploy runs now.
rm -rf -- "${releases}"/.incoming-* "${releases}"/.stale-*
# On the releases' filesystem, so the unpacked release is renamed into place atomically.
work="$(mktemp -d -p "$releases" .incoming-XXXXXX)"

fetch_release() {
  if [[ -f "${target}/${COMPLETE_MARKER}" ]]; then
    echo "release ${build} is already unpacked"
    return
  fi
  echo "downloading ${name}"
  curl -fsSL --retry 3 -o "${work}/${name}.tar.gz" "${BASE_URL}/${name}/${name}.tar.gz"
  curl -fsSL --retry 3 -o "${work}/${name}.tar.gz.sha256" "${BASE_URL}/${name}/${name}.tar.gz.sha256"
  (cd "$work" && sha256sum --check --strict "${name}.tar.gz.sha256")
  mkdir "${work}/unpacked"
  tar -xzf "${work}/${name}.tar.gz" -C "${work}/unpacked" --strip-components=1
  [[ "$(cat "${work}/unpacked/SOURCE_SHA")" == "$build" ]] || { echo "the archive is not release ${build}" >&2; exit 1; }
  : > "${work}/unpacked/${COMPLETE_MARKER}"
  if [[ -e "$target" ]]; then
    # An incomplete directory left by an interrupted deploy: replace it.
    echo "replacing the incomplete ${target}" >&2
    mv -T "$target" "${work}/stale"
  fi
  mv -T "${work}/unpacked" "$target"
}

# The build the symlink names, or empty on a host that has not switched to releases yet.
current_release() {
  [[ -L "$link" ]] && readlink -f "$link" || true
}

# A release's MIGRATIONS as "<name> <sha256>" lines; "-" for releases that listed names only.
migration_entries() {
  awk 'NF == 1 { print $1, "-"; next }
       NF == 2 && $1 ~ /^[0-9a-f]+$/ && length($1) == 64 { print $2, $1; next }
       { exit 1 }' "$1"
}

# The migrations build $1 applies that build $2 lacks or holds with another checksum, one per line.
# A build without MIGRATIONS (legacy) lacks all of them.
migrations_lost() {
  local from to
  from="$(migration_entries "$1/MIGRATIONS")" || { echo "$1/MIGRATIONS is malformed" >&2; return 1; }
  if [[ ! -f "$2/MIGRATIONS" ]]; then
    awk 'NF { print $1 }' <<< "$from"
    return
  fi
  to="$(migration_entries "$2/MIGRATIONS")" || { echo "$2/MIGRATIONS is malformed" >&2; return 1; }
  awk 'FILENAME == ARGV[1] { if (NF) sum[$1] = $2; next }
       NF && (!($1 in sum) || ($2 != "-" && sum[$1] != "-" && $2 != sum[$1])) { print $1 }' \
    <(printf '%s\n' "$to") <(printf '%s\n' "$from")
}

# Crawler-local migrations are applied at startup and never reverted, so an older build cannot run
# on a database that a newer one migrated: refuse that rollback until the operator restored the
# database from a backup taken before those migrations.
check_rollback() {
  local current="$1" missing
  [[ -n "$current" && -f "${current}/MIGRATIONS" ]] || return 0
  missing="$(migrations_lost "$current" "$target")" || exit 1
  if [[ -n "$missing" ]] && ! $db_restored; then
    echo "the running build applied migrations that ${build} lacks or holds with another checksum:" >&2
    echo "$missing" >&2
    echo "restore the crawler database from an S3 backup taken before them, then rerun with --db-restored" >&2
    if $unattended; then exit "$EXIT_NEEDS_OPERATOR"; fi
    exit 1
  fi
}

# Migrations are applied at startup and never reverted, so a build that adds some is deployed by an
# operator. Migrations that are unknown on either side (a host before releases) count as added.
release_adds_migrations() {
  local current="$1" added
  [[ -n "$current" && -f "${current}/MIGRATIONS" && -f "${target}/MIGRATIONS" ]] || return 0
  added="$(migrations_lost "$target" "$current")" || exit 1
  [[ -n "$added" ]]
}

# `/opt/crawler/bin` was a directory of copied binaries before releases: keep it as the rollback
# target of last resort instead of deleting it.
adopt_legacy_bin() {
  if [[ -d "$link" && ! -L "$link" ]]; then
    local legacy
    legacy="${releases}/legacy-$(date -u +%Y%m%dT%H%M%SZ)"
    mv "$link" "$legacy"
    ln -s "$legacy" "$link"
    echo "kept the previous binaries as ${legacy}"
  fi
}

# Also marks the build as the latest deployed: pruning keeps the most recently deployed builds.
switch_release() {
  touch -- "$1"
  ln -sfn "$1" "${link}.new"
  mv -T "${link}.new" "$link"
}

unit_active() {
  systemctl is-active --quiet "$1"
}

unit_installed() {
  [[ "$(systemctl show --property=LoadState --value "$1")" != not-found ]]
}

# Stops both together so the runner never talks to a console of another build. A unit that is not
# installed (a host without the agent runner) is skipped.
stop_services() {
  local unit units=()
  for unit in "$RUNNER_UNIT" "$SERVER_UNIT"; do
    ! unit_installed "$unit" || units+=("$unit")
  done
  services_stopped=true
  ((${#units[@]} == 0)) || systemctl stop "${units[@]}"
  if unit_active "$SERVER_UNIT"; then
    echo "${SERVER_UNIT} is still active after the stop" >&2
    return 1
  fi
}

install_changed_units() {
  local changed=false unit
  # A legacy build carries no units.
  [[ -d "${target}/systemd" ]] || return 0
  for unit in "${UNITS[@]}" "${OPTIONAL_UNITS[@]}"; do
    [[ " ${UNITS[*]} " == *" ${unit} "* || -e "${UNIT_DIR}/${unit}" ]] || continue
    if ! cmp -s "${target}/systemd/${unit}" "${UNIT_DIR}/${unit}"; then
      if $install_units; then
        install -m 0644 "${target}/systemd/${unit}" "${UNIT_DIR}/${unit}"
        echo "installed ${unit}"
        changed=true
      else
        echo "note: ${unit} differs from ${target}/systemd/${unit}; review it and rerun with --install-units to install it" >&2
      fi
    fi
  done
  ! $changed || systemctl daemon-reload
}

wait_healthy() {
  local deadline=$((SECONDS + HEALTH_TIMEOUT))
  while true; do
    if curl -fsS --max-time 5 "$HEALTH_URL" 2>/dev/null | grep -q '"ok":true'; then
      echo "healthy: ${HEALTH_URL}"
      return 0
    fi
    ((SECONDS < deadline)) || break
    sleep "$HEALTH_INTERVAL"
  done
  echo "the crawler is not healthy at ${HEALTH_URL} after ${HEALTH_TIMEOUT}s; see: journalctl -u ${SERVER_UNIT}" >&2
  return 1
}

prune_releases() {
  local keep_current keep_previous="" dir name
  keep_current="$(basename "$1")"
  [[ -z "$2" ]] || keep_previous="$(basename "$2")"
  # Most recently deployed first (switch_release touches each build it switches to); the running
  # build, the build it replaced and the legacy fallback are never pruned.
  ls -1dt "${releases}"/*/ 2>/dev/null | sed 's:/$::' | tail -n +"$((KEEP + 1))" | while read -r dir; do
    name="$(basename "$dir")"
    if [[ "$name" != "$keep_current" && "$name" != "$keep_previous" && "$name" != legacy-* ]]; then
      rm -rf -- "$dir"
    fi
  done
}

# How to return to the build this deploy replaced.
rollback_hint() {
  local to="$1" lost
  if [[ -z "$to" || "$to" == "$target" ]]; then
    echo "no earlier build to roll back to on this host" >&2
    return
  fi
  lost="$(migrations_lost "$target" "$to" 2>/dev/null)" || lost=unknown
  if [[ -n "$lost" ]]; then
    echo "roll back: restore the crawler database from the last S3 backup, then run: ${0##*/} --db-restored $(basename "$to")" >&2
  else
    echo "roll back with: ${0##*/} $(basename "$to")" >&2
  fi
}

if $legacy; then
  [[ -d "$target" ]] || { echo "no kept build ${target}" >&2; exit 1; }
else
  fetch_release
fi
previous="$(current_release)"
if [[ "$previous" == "$target" ]]; then
  echo "build ${build} is already running; restarting it"
fi
[[ "$previous" == "$target" ]] || check_rollback "$previous"
if $unattended && release_adds_migrations "$previous"; then
  echo "${build} adds crawler-local migrations: deploy it by hand: back the database up first (systemctl start crawler-backup.service) and watch it" >&2
  exit "$EXIT_NEEDS_OPERATOR"
fi

unit_active "$RUNNER_UNIT" && runner_was_active=true

stop_services
# A failure from here on starts the stopped services again (on_exit).
adopt_legacy_bin
# The build this one replaces, including binaries adopted as legacy-<time> just now.
rollback_to="$(current_release)"
switch_release "$target"
[[ -z "$rollback_to" || "$rollback_to" == "$target" ]] || switched_from="$rollback_to"
install_changed_units
start_services
switched_from=""

if wait_healthy; then
  prune_releases "$target" "$rollback_to"
  echo "deployed ${build}"
else
  if $unattended && [[ -n "$rollback_to" && "$rollback_to" != "$target" ]]; then
    # No migration was applied (refused above), so the previous build runs on the same database.
    echo "rolling back to $(basename "$rollback_to")" >&2
    stop_services
    switch_release "$rollback_to"
    start_services
    wait_healthy || echo "the previous build is not healthy either" >&2
  else
    rollback_hint "$rollback_to"
  fi
  exit "$EXIT_UNHEALTHY"
fi
