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
cat > "$tmp/fakebin/systemctl" <<'T'
#!/usr/bin/env bash
echo "systemctl $*" >> "$FAKE_LOG"
[[ "$1" == is-active ]] && exit "${FAKE_RUNNER_ACTIVE:-0}"
exit 0
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
cp "$FAKE_RELEASES/$(basename "$url")" "$out"
T
cat > "$tmp/fakebin/pg_dump" <<'T'
#!/usr/bin/env bash
[[ -z "${FAKE_PG_FAIL:-}" ]] || exit 1
for a in "$@"; do [[ "$a" == --file=* ]] && echo dump > "${a#--file=}"; done
T
chmod +x "$tmp"/fakebin/*
export PATH="$tmp/fakebin:$PATH" FAKE_LOG="$tmp/log/calls" FAKE_RELEASES="$tmp/releases-out"
export FAKE_HEALTH_FAILS="$tmp/log/health-fails" FAKE_API_JSON="$tmp/log/api.json"

for bin in server review-mcp review-agent-runner; do echo "$bin" > "$tmp/build/$bin"; done
bash "$SCRIPT_DIR/package-release.sh" "$sha_a" "$tmp/build" "$tmp/releases-out" >/dev/null
bash "$SCRIPT_DIR/package-release.sh" "$sha_b" "$tmp/build" "$tmp/releases-out" >/dev/null
first_sum="$(cat "$tmp/releases-out/crawler-$sha_a.tar.gz.sha256")"
bash "$SCRIPT_DIR/package-release.sh" "$sha_a" "$tmp/build" "$tmp/releases-out" >/dev/null
[[ "$first_sum" == "$(cat "$tmp/releases-out/crawler-$sha_a.tar.gz.sha256")" ]] || fail "package is not reproducible"

echo 'LOCAL_DB_URL=postgres://u:p@127.0.0.1/db' > "$tmp/crawler.env"
export CRAWLER_ROOT="$tmp/opt" CRAWLER_RELEASE_BASE_URL="https://example.test/dl" \
  CRAWLER_ENV_FILE="$tmp/crawler.env" CRAWLER_BACKUP_DIR="$tmp/backups" CRAWLER_UNIT_DIR="$tmp/units" \
  CRAWLER_HEALTH_ATTEMPTS=1 CRAWLER_HEALTH_INTERVAL=0
mkdir -p "$tmp/units" "$CRAWLER_ROOT/bin"; echo legacy > "$CRAWLER_ROOT/bin/server"
deploy() { bash "$SCRIPT_DIR/deploy.sh" "$@"; }

deploy "$sha_a" >/dev/null
[[ "$(readlink -f "$CRAWLER_ROOT/bin")" == "$CRAWLER_ROOT/releases/$sha_a" ]] || fail "symlink not switched"
[[ -x "$CRAWLER_ROOT/bin/server" ]] || fail "binary missing behind symlink"
ls "$CRAWLER_ROOT"/releases/legacy-* >/dev/null || fail "legacy binaries not kept"
[[ "$(ls "$tmp/backups" | wc -l)" -eq 1 ]] || fail "a host without releases must be backed up"

deploy "$sha_b" >/dev/null
[[ "$(ls "$tmp/backups" | wc -l)" -eq 1 ]] || fail "a release without new migrations must take no backup"
grep -q "stop review-agent-runner crawler-server" "$FAKE_LOG" || fail "services not stopped together"

# A migration the running build applied and the target lacks blocks the rollback; the rollback
# itself adds no migration and takes no backup.
echo 99990101000000_new.sql >> "$CRAWLER_ROOT/releases/$sha_b/MIGRATIONS"
if deploy "$sha_a" >/dev/null 2>&1; then fail "rollback over a migration was allowed"; fi
deploy --db-restored "$sha_a" >/dev/null
[[ "$(ls "$tmp/backups" | wc -l)" -eq 1 ]] || fail "a rollback must take no backup"

# A failed backup aborts a release that adds a migration before the switch and leaves the running
# build up.
: > "$FAKE_LOG"
if FAKE_PG_FAIL=1 deploy "$sha_b" >/dev/null 2>&1; then fail "deploy went on without a backup"; fi
[[ "$(readlink -f "$CRAWLER_ROOT/bin")" == "$CRAWLER_ROOT/releases/$sha_a" ]] || fail "switched despite the failed backup"
grep -q "start crawler-server" "$FAKE_LOG" || fail "running build not restarted after failed backup"
ls "$tmp/backups"/*.partial >/dev/null 2>&1 && fail "partial dump left behind"

# A release that adds a migration is backed up before it is switched in.
deploy "$sha_b" >/dev/null
[[ "$(ls "$tmp/backups" | wc -l)" -eq 2 ]] || fail "a release adding a migration must be backed up"

# A tampered archive is refused.
echo tampered >> "$tmp/releases-out/crawler-$sha_b.tar.gz"; rm -rf "$CRAWLER_ROOT/releases/$sha_b"
if deploy "$sha_b" >/dev/null 2>&1; then fail "checksum mismatch accepted"; fi

# --- Automatic deploys: autodeploy.sh and deploy.sh --unattended on a fresh host root. ---
sha_c="$(printf 'c%.0s' {1..40})"
sha_d="$(printf 'd%.0s' {1..40})"
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

echo "deploy.test.sh: all checks passed"
