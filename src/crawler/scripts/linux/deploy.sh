#!/usr/bin/env bash
# Deploys a released crawler build on this host, or rolls back to an earlier one: run it with the
# commit SHA of a published crawler-<sha> GitHub Release. Pull-based: the host downloads the
# release, so CI holds no host credentials, and needs neither Git, Cargo nor a source tree.
# A release that adds crawler-local migrations is preceded by a dump of that database; a failed
# dump aborts the deploy.
# --unattended is for the autodeploy timer: it refuses (exit 3) a release that adds migrations,
# which needs an operator, and rolls an unhealthy release back (exit 4) to the build it replaced.
# Usage: deploy.sh [--install-units] [--db-restored] [--unattended] <full lowercase sha>
# Contract: docs/crawler/operations.md#deploy-and-roll-back
set -euo pipefail

ROOT="${CRAWLER_ROOT:-/opt/crawler}"
REPOSITORY="${CRAWLER_RELEASE_REPOSITORY:-aura-historia/backend}"
BASE_URL="${CRAWLER_RELEASE_BASE_URL:-https://github.com/${REPOSITORY}/releases/download}"
HEALTH_URL="${CRAWLER_HEALTH_URL:-http://127.0.0.1:7878/health}"
HEALTH_ATTEMPTS="${CRAWLER_HEALTH_ATTEMPTS:-30}"
HEALTH_INTERVAL="${CRAWLER_HEALTH_INTERVAL:-2}"
KEEP="${CRAWLER_KEEP_RELEASES:-5}"
ENV_FILE="${CRAWLER_ENV_FILE:-/etc/crawler/crawler.env}"
BACKUP_DIR="${CRAWLER_BACKUP_DIR:-/var/backups/crawler}"
KEEP_BACKUPS="${CRAWLER_KEEP_BACKUPS:-10}"
UNIT_DIR="${CRAWLER_UNIT_DIR:-/etc/systemd/system}"
readonly SERVER_UNIT=crawler-server
readonly RUNNER_UNIT=review-agent-runner
readonly EXIT_NEEDS_OPERATOR=3
readonly EXIT_UNHEALTHY=4
readonly UNITS=("${SERVER_UNIT}.service" "${RUNNER_UNIT}.service")
# Present in a release; managed only on a host that installed them.
readonly OPTIONAL_UNITS=(crawler-autodeploy.service crawler-autodeploy.timer)

install_units=false
db_restored=false
unattended=false
sha=""
for arg in "$@"; do
  case "$arg" in
    --install-units) install_units=true ;;
    --db-restored) db_restored=true ;;
    --unattended) unattended=true ;;
    -*) echo "unknown option: $arg" >&2; exit 2 ;;
    *) [[ -z "$sha" ]] || { echo "give exactly one SHA" >&2; exit 2; }; sha="$arg" ;;
  esac
done
[[ "$sha" =~ ^[a-f0-9]{40}$ ]] || { echo "usage: ${0##*/} [--install-units] [--db-restored] [--unattended] <full lowercase sha>" >&2; exit 2; }

releases="${ROOT}/releases"
target="${releases}/${sha}"
link="${ROOT}/bin"
name="crawler-${sha}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# One deploy at a time, whether started by hand or by the timer.
mkdir -p "$ROOT"
exec 9> "${ROOT}/deploy.lock"
flock -n 9 || { echo "another deploy is running" >&2; exit 1; }

fetch_release() {
  mkdir -p "$releases"
  if [[ -d "$target" ]]; then
    echo "release ${sha} is already unpacked"
    return
  fi
  echo "downloading ${name}"
  curl -fsSL --retry 3 -o "${work}/${name}.tar.gz" "${BASE_URL}/${name}/${name}.tar.gz"
  curl -fsSL --retry 3 -o "${work}/${name}.tar.gz.sha256" "${BASE_URL}/${name}/${name}.tar.gz.sha256"
  (cd "$work" && sha256sum --check --strict "${name}.tar.gz.sha256")
  mkdir "${work}/unpacked"
  tar -xzf "${work}/${name}.tar.gz" -C "${work}/unpacked" --strip-components=1
  [[ "$(cat "${work}/unpacked/SOURCE_SHA")" == "$sha" ]] || { echo "the archive is not release ${sha}" >&2; exit 1; }
  mv "${work}/unpacked" "$target"
}

# The release the symlink names, or empty on a host that has not switched to releases yet.
current_release() {
  [[ -L "$link" ]] && readlink -f "$link" || true
}

# Crawler-local migrations are applied at startup and never reverted, so an older build cannot run
# on a database that a newer one migrated: refuse that rollback until the operator restored the
# database from a backup taken before those migrations.
check_rollback() {
  local current="$1" missing
  [[ -n "$current" && -f "${current}/MIGRATIONS" ]] || return 0
  missing="$(comm -23 "${current}/MIGRATIONS" "${target}/MIGRATIONS")"
  if [[ -n "$missing" ]] && ! $db_restored; then
    echo "the running build applied migrations that ${sha} does not contain:" >&2
    echo "$missing" >&2
    echo "restore the crawler database from a backup taken before them, then rerun with --db-restored" >&2
    exit 1
  fi
}

# Migrations are applied at startup and never reverted, so only a build that adds some needs a
# backup. A running build whose migrations are unknown (a host before releases) counts as adding.
release_adds_migrations() {
  local current="$1"
  [[ -n "$current" && -f "${current}/MIGRATIONS" ]] || return 0
  [[ -n "$(comm -13 "${current}/MIGRATIONS" "${target}/MIGRATIONS")" ]]
}

# Dumps the crawler-local database (`LOCAL_DB_URL` of the crawler's environment file) with the
# services stopped, so the dump is exactly the state the new build starts from.
backup_database() {
  local url dump
  url="$(sed -n 's/^LOCAL_DB_URL=//p' "$ENV_FILE" | head -n1 | sed -e 's/^"//' -e 's/"$//' -e "s/^'//" -e "s/'\$//")"
  [[ -n "$url" ]] || { echo "LOCAL_DB_URL is not set in ${ENV_FILE}" >&2; return 1; }
  install -d -m 0700 "$BACKUP_DIR"
  dump="${BACKUP_DIR}/crawler-$(date -u +%Y%m%dT%H%M%SZ)-before-${sha}.dump"
  # Written under a temporary name: a dump that failed halfway is never mistaken for a backup.
  if ! (umask 0077 && pg_dump --format=custom --no-owner --dbname="$url" --file="${dump}.partial"); then
    rm -f -- "${dump}.partial"
    echo "the database backup failed" >&2
    return 1
  fi
  mv "${dump}.partial" "$dump"
  echo "backed up the crawler database to ${dump}"
  ls -1t "${BACKUP_DIR}"/crawler-*.dump 2>/dev/null | tail -n +"$((KEEP_BACKUPS + 1))" | xargs -r rm -f --
}

# `/opt/crawler/bin` was a directory of copied binaries before releases: keep it as the rollback
# target of last resort instead of deleting it.
adopt_legacy_bin() {
  if [[ -d "$link" && ! -L "$link" ]]; then
    local legacy="${releases}/legacy-$(date -u +%Y%m%dT%H%M%SZ)"
    mv "$link" "$legacy"
    ln -s "$legacy" "$link"
    echo "kept the previous binaries as ${legacy}"
  fi
}

switch_release() {
  ln -sfn "$1" "${link}.new"
  mv -T "${link}.new" "$link"
}

unit_active() {
  systemctl is-active --quiet "$1"
}

install_changed_units() {
  local changed=false unit
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
  local attempt
  for ((attempt = 1; attempt <= HEALTH_ATTEMPTS; attempt++)); do
    if curl -fsS --max-time 5 "$HEALTH_URL" 2>/dev/null | grep -q '"ok":true'; then
      echo "healthy: ${HEALTH_URL}"
      return 0
    fi
    sleep "$HEALTH_INTERVAL"
  done
  echo "the crawler is not healthy at ${HEALTH_URL}; see: journalctl -u ${SERVER_UNIT}" >&2
  return 1
}

prune_releases() {
  local keep_current dir
  keep_current="$(readlink -f "$link")"
  # Newest first; the running release and the legacy fallback are never pruned.
  ls -1dt "${releases}"/*/ 2>/dev/null | sed 's:/$::' | tail -n +"$((KEEP + 1))" | while read -r dir; do
    [[ "$dir" != "$keep_current" && "$(basename "$dir")" != legacy-* ]] && rm -rf -- "$dir"
  done
}

fetch_release
previous="$(current_release)"
if [[ "$previous" == "$target" ]]; then
  echo "release ${sha} is already running; restarting it"
fi
[[ "$previous" == "$target" ]] || check_rollback "$previous"
if $unattended && release_adds_migrations "$previous"; then
  echo "${sha} adds crawler-local migrations: deploy it by hand so the database is backed up and watched" >&2
  exit "$EXIT_NEEDS_OPERATOR"
fi

runner_was_active=false
unit_active "$RUNNER_UNIT" && runner_was_active=true

# Stop both together so the runner never talks to a console of another build.
systemctl stop "$RUNNER_UNIT" "$SERVER_UNIT"
if release_adds_migrations "$previous" && ! backup_database; then
  # Nothing was switched: bring the running build back up and stop.
  systemctl start "$SERVER_UNIT"
  ! $runner_was_active || systemctl start "$RUNNER_UNIT"
  exit 1
fi
adopt_legacy_bin
switch_release "$target"
install_changed_units
systemctl start "$SERVER_UNIT"
! $runner_was_active || systemctl start "$RUNNER_UNIT"

if wait_healthy; then
  prune_releases
  echo "deployed ${sha}"
else
  if $unattended && [[ -n "$previous" ]]; then
    # No migration was applied (refused above), so the previous build runs on the same database.
    echo "rolling back to $(basename "$previous")" >&2
    systemctl stop "$RUNNER_UNIT" "$SERVER_UNIT"
    switch_release "$previous"
    systemctl start "$SERVER_UNIT"
    ! $runner_was_active || systemctl start "$RUNNER_UNIT"
    wait_healthy || echo "the previous build is not healthy either" >&2
  else
    echo "roll back with: ${0##*/} ${previous:+$(basename "$previous")}" >&2
  fi
  exit "$EXIT_UNHEALTHY"
fi
