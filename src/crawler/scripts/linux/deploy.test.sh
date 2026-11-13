#!/usr/bin/env bash
# Exercises package-release.sh and deploy.sh against fake systemctl, curl and pg_dump.
# Run: bash src/crawler/scripts/linux/deploy.test.sh
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
fail() { echo "FAIL: $*" >&2; cat "$tmp/log/autodeploy.out" >&2 2>/dev/null || true; exit 1; }

sha_a="$(printf 'a%.0s' {1..40})"
sha_b="$(printf 'b%.0s' {1..40})"

# Fake host tools: systemctl logs calls, curl serves the release files and the health check.
mkdir -p "$tmp/fakebin" "$tmp/build" "$tmp/releases-out" "$tmp/log"
# The fake systemctl keeps each unit's state as a file in $FAKE_UNITS: present means active.
# FAKE_MISSING_UNITS names units that are not installed, FAKE_STUCK_UNITS units that ignore a stop,
# and FAKE_FAIL_DAEMON_RELOAD makes daemon-reload fail.
cat > "$tmp/fakebin/systemctl" <<'T'
#!/usr/bin/env bash
echo "systemctl $*" >> "$FAKE_LOG"
cmd="$1"; shift
listed() { [[ " $2 " == *" ${1%.service} "* ]]; }
case "$cmd" in
  show) if listed "${!#}" "${FAKE_MISSING_UNITS:-}"; then echo not-found; else echo loaded; fi ;;
  is-active) [[ -f "$FAKE_UNITS/${!#%.service}" ]] ;;
  stop|start)
    for unit in "$@"; do ! listed "$unit" "${FAKE_MISSING_UNITS:-}" || exit 5; done
    for unit in "$@"; do
      if [[ "$cmd" == start ]]; then touch "$FAKE_UNITS/${unit%.service}"
      elif ! listed "$unit" "${FAKE_STUCK_UNITS:-}"; then rm -f "$FAKE_UNITS/${unit%.service}"; fi
    done ;;
  daemon-reload) [[ -z "${FAKE_FAIL_DAEMON_RELOAD:-}" ]] ;;
esac
T
cat > "$tmp/fakebin/curl" <<'T'
#!/usr/bin/env bash
out=""; url=""
while [[ $# -gt 0 ]]; do case "$1" in -o) out="$2"; shift 2 ;; -*) shift ;; *) url="$1"; shift ;; esac; done
if [[ "$url" == */health ]]; then
  fails="$(cat "$FAKE_HEALTH_FAILS" 2>/dev/null || echo 0)"
  if [[ "$fails" -gt 0 ]]; then echo $((fails - 1)) > "$FAKE_HEALTH_FAILS"; exit 22; fi
  echo '{"ok":true}'; exit 0
fi
if [[ "$url" == */api ]]; then cat "$FAKE_API_JSON"; exit 0; fi
# The compare API: FAKE_COMMIT_ORDER lists the test SHAs in develop's commit order.
if [[ "$url" == */compare/* ]]; then
  [[ -z "${FAKE_COMPARE_FAIL:-}" ]] || exit 22
  pair="${url##*/compare/}"; pair="${pair%%\?*}"
  base_index=-1; head_index=-1; i=0
  for sha in $FAKE_COMMIT_ORDER; do
    [[ "$sha" != "${pair%%...*}" ]] || base_index=$i
    [[ "$sha" != "${pair##*...}" ]] || head_index=$i
    i=$((i + 1))
  done
  ahead=0; behind=0
  if ((head_index > base_index)); then ahead=$((head_index - base_index)); else behind=$((base_index - head_index)); fi
  printf '{"status": "x", "ahead_by": %s, "behind_by": %s, "files": [{"status": "modified"}]}\n' "$ahead" "$behind"
  exit 0
fi
cp "$FAKE_RELEASES/$(basename "$url")" "$out"
T
cat > "$tmp/fakebin/pg_dump" <<'T'
#!/usr/bin/env bash
# deploy.sh takes no backup of its own: the weekly S3 backup is the only one.
echo called > "$FAKE_PG_CALLED"
T
chmod +x "$tmp"/fakebin/*
export PATH="$tmp/fakebin:$PATH" FAKE_LOG="$tmp/log/calls" FAKE_RELEASES="$tmp/releases-out"
export FAKE_PG_CALLED="$tmp/log/pg_dump-called" FAKE_HEALTH_FAILS="$tmp/log/health-fails" FAKE_API_JSON="$tmp/log/api.json" FAKE_UNITS="$tmp/state"
mkdir -p "$FAKE_UNITS"; touch "$FAKE_UNITS/crawler-server" "$FAKE_UNITS/review-agent-runner"
active() { [[ -f "$FAKE_UNITS/$1" ]]; }

for bin in server review-mcp review-agent-runner; do echo "$bin" > "$tmp/build/$bin"; done
bash "$SCRIPT_DIR/package-release.sh" "$sha_a" "$tmp/build" "$tmp/releases-out" >/dev/null
bash "$SCRIPT_DIR/package-release.sh" "$sha_b" "$tmp/build" "$tmp/releases-out" >/dev/null
first_sum="$(cat "$tmp/releases-out/crawler-$sha_a.tar.gz.sha256")"
bash "$SCRIPT_DIR/package-release.sh" "$sha_a" "$tmp/build" "$tmp/releases-out" >/dev/null
[[ "$first_sum" == "$(cat "$tmp/releases-out/crawler-$sha_a.tar.gz.sha256")" ]] || fail "package is not reproducible"
tar -xzOf "$tmp/releases-out/crawler-$sha_a.tar.gz" "crawler-$sha_a/MIGRATIONS" > "$tmp/log/migrations"
if [[ ! -s "$tmp/log/migrations" ]] || grep -Evq '^[a-f0-9]{64}  [0-9]{14}_[a-z0-9_]+\.sql$' "$tmp/log/migrations"; then
  fail "MIGRATIONS does not record a sha256 per migration"
fi

echo 'LOCAL_DB_URL=postgres://u:p@127.0.0.1/db' > "$tmp/crawler.env"
export CRAWLER_ROOT="$tmp/opt" CRAWLER_RELEASE_BASE_URL="https://example.test/dl" \
  CRAWLER_ENV_FILE="$tmp/crawler.env" CRAWLER_UNIT_DIR="$tmp/units" \
  CRAWLER_HEALTH_TIMEOUT=0 CRAWLER_HEALTH_INTERVAL=0
mkdir -p "$tmp/units" "$CRAWLER_ROOT/bin"; echo legacy > "$CRAWLER_ROOT/bin/server"
deploy() { bash "$SCRIPT_DIR/deploy.sh" "$@"; }

deploy "$sha_a" >/dev/null
[[ "$(readlink -f "$CRAWLER_ROOT/bin")" == "$CRAWLER_ROOT/releases/$sha_a" ]] || fail "symlink not switched"
[[ -x "$CRAWLER_ROOT/bin/server" ]] || fail "binary missing behind symlink"
ls "$CRAWLER_ROOT"/releases/legacy-* >/dev/null || fail "legacy binaries not kept"

deploy "$sha_b" >/dev/null
grep -q "stop review-agent-runner crawler-server" "$FAKE_LOG" || fail "services not stopped together"

# A migration the running build applied and the target lacks blocks the rollback.
# Releases published before checksums listed names only: they are still read.
awk '{ print $2 }' "$CRAWLER_ROOT/releases/$sha_b/MIGRATIONS" > "$tmp/log/names" && mv "$tmp/log/names" "$CRAWLER_ROOT/releases/$sha_b/MIGRATIONS"
echo 99990101000000_new.sql >> "$CRAWLER_ROOT/releases/$sha_b/MIGRATIONS"
if deploy "$sha_a" >/dev/null 2>&1; then fail "rollback over a migration was allowed"; fi
deploy --db-restored "$sha_a" >/dev/null

# A release that adds a migration is switched in without a dump of its own.
deploy "$sha_b" >/dev/null

# A tampered archive is refused.
echo tampered >> "$tmp/releases-out/crawler-$sha_b.tar.gz"; rm -rf "$CRAWLER_ROOT/releases/$sha_b"
if deploy "$sha_b" >/dev/null 2>&1; then fail "checksum mismatch accepted"; fi

# --- Automatic deploys: autodeploy.sh and deploy.sh --unattended on a fresh host root. ---
sha_c="$(printf 'c%.0s' {1..40})"
sha_d="$(printf 'd%.0s' {1..40})"
sha_e="$(printf 'e%.0s' {1..40})"
sha_f="$(printf 'f%.0s' {1..40})"; sha_g="$(printf '1%.0s' {1..40})"; sha_h="$(printf '2%.0s' {1..40})"
export FAKE_COMMIT_ORDER="$sha_a $sha_b $sha_c $sha_d $sha_e $sha_f $sha_g $sha_h"
bash "$SCRIPT_DIR/package-release.sh" "$sha_b" "$tmp/build" "$tmp/releases-out" >/dev/null  # the tamper test corrupted it
bash "$SCRIPT_DIR/package-release.sh" "$sha_c" "$tmp/build" "$tmp/releases-out" >/dev/null
# A release that adds a crawler-local migration, packaged from a copy of the crawler directory.
alt="$tmp/alt"; mkdir -p "$alt/scripts"
cp -r "$SCRIPT_DIR/../../migrations" "$alt/migrations"; cp -r "$SCRIPT_DIR" "$alt/scripts/linux"
touch "$alt/migrations/99990101000000_extra.sql"
bash "$alt/scripts/linux/package-release.sh" "$sha_d" "$tmp/build" "$tmp/releases-out" >/dev/null

export CRAWLER_ROOT="$tmp/opt2" CRAWLER_RELEASES_API_URL="https://example.test/api"
mkdir -p "$CRAWLER_ROOT"
api() { for sha in "$@"; do printf '{"tag_name": "crawler-%s"},\n' "$sha"; done > "$FAKE_API_JSON"; }
bin_is() { [[ "$(readlink -f "$CRAWLER_ROOT/bin")" == "$CRAWLER_ROOT/releases/$1" ]]; }
autodeploy() { bash "$SCRIPT_DIR/autodeploy.sh" > "$tmp/log/autodeploy.out" 2>&1; }

deploy "$sha_a" >/dev/null 2>&1
bin_is "$sha_a" || fail "setup of the autodeploy host failed"
[[ -x "$CRAWLER_ROOT/bin/autodeploy.sh" && -f "$CRAWLER_ROOT/bin/systemd/crawler-autodeploy.timer" ]] || fail "release lacks the autodeploy files"

api "$sha_b" "$sha_a"
autodeploy || fail "autodeploy of a newer release failed"
bin_is "$sha_b" || fail "autodeploy did not deploy the newest release"
: > "$FAKE_LOG"; autodeploy
[[ ! -s "$FAKE_LOG" ]] || fail "autodeploy acted although the newest release runs"

# After a manual rollback the timer stays quiet until a newer release is published.
deploy --db-restored "$sha_a" >/dev/null 2>&1
: > "$FAKE_LOG"; autodeploy
bin_is "$sha_a" || fail "autodeploy undid a manual rollback"

# An unhealthy release is rolled back and left to an operator.
api "$sha_c" "$sha_b"
echo 1 > "$FAKE_HEALTH_FAILS"
if autodeploy; then fail "an unhealthy release was reported as deployed"; fi
bin_is "$sha_a" || fail "an unhealthy release was not rolled back"
[[ "$(cat "$CRAWLER_ROOT/autodeploy.handled")" == "$sha_c" ]] || fail "an unhealthy release would be retried"

# A release that adds migrations is never deployed unattended.
api "$sha_d" "$sha_c"
if autodeploy; then fail "a release with migrations was deployed unattended"; fi
bin_is "$sha_a" || fail "switched to a release with migrations"
[[ "$(cat "$CRAWLER_ROOT/autodeploy.handled")" == "$sha_d" ]] || fail "a release with migrations would be retried"
deploy "$sha_d" >/dev/null 2>&1
bin_is "$sha_d" || fail "an operator could not deploy the release with migrations"


# Unattended mode leaves a rollback over applied migrations to an operator (exit 3), so autodeploy
# does not retry it like a lock conflict.
status=0; deploy --unattended "$sha_a" >/dev/null 2>&1 || status=$?
[[ "$status" -eq 3 ]] || fail "an unattended rollback over applied migrations exited ${status}, not 3"
bin_is "$sha_d" || fail "an unattended rollback over applied migrations switched"

# Migration checksums: a release that changes an applied migration cannot run on the migrated
# database, so it is refused in both directions until the database is restored, never deployed
# unattended.
echo 'SELECT 1;' > "$alt/migrations/99990101000000_extra.sql"
bash "$alt/scripts/linux/package-release.sh" "$sha_e" "$tmp/build" "$tmp/releases-out" >/dev/null
status=0; deploy --unattended "$sha_e" >/dev/null 2>&1 || status=$?
[[ "$status" -eq 3 ]] || fail "a release that changes a migration was deployed unattended (exit ${status})"
if deploy "$sha_e" >/dev/null 2>&1; then fail "a release that changes an applied migration was deployed"; fi
deploy --db-restored "$sha_e" >/dev/null 2>&1 || fail "the release that changes a migration was not deployed"
if deploy "$sha_d" >/dev/null 2>&1; then fail "a rollback over a changed migration was allowed"; fi
bin_is "$sha_e" || fail "a refused rollback switched"

# The health check waits CRAWLER_HEALTH_TIMEOUT seconds, long enough for startup migrations.
echo 40 > "$FAKE_HEALTH_FAILS"
CRAWLER_HEALTH_TIMEOUT=60 deploy "$sha_e" >/dev/null 2>&1 || fail "a build that became healthy within CRAWLER_HEALTH_TIMEOUT was rolled back"
rm -f "$FAKE_HEALTH_FAILS"

# The timer never downgrades: a release of an older commit, published later, is skipped once,
# and a failed comparison is retried.
api "$sha_c" "$sha_e"
: > "$FAKE_LOG"
FAKE_COMPARE_FAIL=1 autodeploy && fail "autodeploy went on without comparing the commits"
[[ "$(cat "$CRAWLER_ROOT/autodeploy.handled")" != "$sha_c" ]] || fail "a failed comparison would not be retried"
autodeploy || fail "skipping an older release failed the unit"
bin_is "$sha_e" || fail "autodeploy downgraded to an older commit"
! grep -q "stop" "$FAKE_LOG" || fail "autodeploy stopped the services for an older commit"
[[ "$(cat "$CRAWLER_ROOT/autodeploy.handled")" == "$sha_c" ]] || fail "an older release would be compared again every run"

# A host without the agent runner stops and starts only the server.
rm -f "$FAKE_UNITS/review-agent-runner"
FAKE_MISSING_UNITS=review-agent-runner deploy "$sha_e" >/dev/null 2>&1 || fail "a deploy failed on a host without review-agent-runner.service"
active crawler-server || fail "crawler-server not started on a host without the agent runner"
touch "$FAKE_UNITS/review-agent-runner"

# A server that does not stop is not switched under, and is left running.
if FAKE_STUCK_UNITS=crawler-server deploy --db-restored "$sha_d" >/dev/null 2>&1; then fail "deployed while crawler-server was still active"; fi
bin_is "$sha_e" || fail "switched while crawler-server was still active"

# A failure between stop and start (here daemon-reload) starts the stopped services again.
if FAKE_FAIL_DAEMON_RELOAD=1 deploy --install-units "$sha_e" >/dev/null 2>&1; then fail "a failed daemon-reload was ignored"; fi
if ! active crawler-server || ! active review-agent-runner; then fail "the services stayed stopped after a failed deploy"; fi

# --- A legacy host: rollback to the adopted binaries, partial releases and pruning. ---
export CRAWLER_ROOT="$tmp/opt3"
mkdir -p "$CRAWLER_ROOT/bin"; echo legacy > "$CRAWLER_ROOT/bin/server"
echo 1 > "$FAKE_HEALTH_FAILS"
status=0; deploy "$sha_a" > "$tmp/log/legacy.out" 2>&1 || status=$?
[[ "$status" -eq 4 ]] || fail "an unhealthy first deploy exited ${status}, not 4"
legacy_name="$(basename "$(ls -d "$CRAWLER_ROOT"/releases/legacy-*)")"
grep -q -- "--db-restored ${legacy_name}" "$tmp/log/legacy.out" || fail "the first deploy on a legacy host printed no usable rollback hint"
if deploy "$legacy_name" >/dev/null 2>&1; then fail "a rollback to binaries with unknown migrations was allowed"; fi
deploy --db-restored "$legacy_name" >/dev/null 2>&1 || fail "the rollback to the legacy binaries failed"
bin_is "$legacy_name" && [[ "$(cat "$CRAWLER_ROOT/bin/server")" == legacy ]] || fail "not rolled back to the legacy binaries"

# A release directory an interrupted deploy left without its completion marker is fetched again,
# and an abandoned download directory is removed.
mkdir -p "$CRAWLER_ROOT/releases/$sha_b" "$CRAWLER_ROOT/releases/.incoming-old"
echo "$sha_b" > "$CRAWLER_ROOT/releases/$sha_b/SOURCE_SHA"
deploy "$sha_b" >/dev/null 2>&1 || fail "the deploy over a partial release failed"
[[ -x "$CRAWLER_ROOT/bin/server" && -f "$CRAWLER_ROOT/bin/.complete" ]] || fail "a partial release directory was trusted"
if compgen -G "$CRAWLER_ROOT/releases/.*-*" >/dev/null; then fail "temporary release directories were left behind"; fi

# More than five releases: the least recently deployed go, the legacy binaries stay, and a healthy
# deploy exits 0 even when the last directory pruning looks at is kept. $sha_a was unpacked before
# $sha_b but is deployed again after it, so $sha_b goes.
for sha in "$sha_f" "$sha_g" "$sha_h"; do bash "$SCRIPT_DIR/package-release.sh" "$sha" "$tmp/build" "$tmp/releases-out" >/dev/null; done
touch -d 2020-01-01 "$CRAWLER_ROOT/releases/$legacy_name"
for sha in "$sha_a" "$sha_c" "$sha_f" "$sha_g" "$sha_d"; do
  deploy "$sha" > "$tmp/log/prune.out" 2>&1 || { cat "$tmp/log/prune.out" >&2; fail "a healthy deploy of ${sha} failed while pruning"; }
done
[[ "$(ls -d "$CRAWLER_ROOT"/releases/*/ | wc -l)" -eq 6 && -d "$CRAWLER_ROOT/releases/$legacy_name" ]] || fail "pruning kept the wrong releases"
[[ ! -d "$CRAWLER_ROOT/releases/$sha_b" ]] || fail "the least recently deployed release was not pruned"

# Pruning follows deploys, not unpacking: after a rollback to the oldest unpacked release and a
# new deploy, the rollback target stays and the least recently deployed release goes.
deploy --db-restored "$sha_a" >/dev/null 2>&1 || fail "the rollback to ${sha_a} failed"
deploy "$sha_h" >/dev/null 2>&1 || fail "the deploy after the rollback failed"
[[ -d "$CRAWLER_ROOT/releases/$sha_a" ]] || fail "the build a deploy replaced was pruned"
[[ ! -d "$CRAWLER_ROOT/releases/$sha_c" ]] || fail "the least recently deployed release was kept"
[[ -d "$CRAWLER_ROOT/releases/$legacy_name" ]] || fail "the legacy binaries were pruned"

# The shipped timer's only trigger is OnCalendar, so a drop-in can replace the schedule.
timer="$SCRIPT_DIR/crawler-autodeploy.timer"
if ! grep -q '^OnCalendar=' "$timer" || grep -Eq '^On(Boot|Startup|Active|UnitActive|UnitInactive)Sec=' "$timer"; then
  fail "the timer has triggers besides OnCalendar"
fi

[[ ! -e "$FAKE_PG_CALLED" ]] || fail "deploy.sh dumped the database; the weekly S3 backup is the only backup"

echo "deploy.test.sh: all checks passed"
