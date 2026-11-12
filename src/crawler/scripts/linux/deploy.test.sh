#!/usr/bin/env bash
# Exercises package-release.sh and deploy.sh against fake systemctl, curl and pg_dump.
# Run: bash src/crawler/scripts/linux/deploy.test.sh
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }

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
if [[ "$url" == */health ]]; then echo '{"ok":true}'; exit 0; fi
cp "$FAKE_RELEASES/$(basename "$url")" "$out"
T
cat > "$tmp/fakebin/pg_dump" <<'T'
#!/usr/bin/env bash
[[ -z "${FAKE_PG_FAIL:-}" ]] || exit 1
for a in "$@"; do [[ "$a" == --file=* ]] && echo dump > "${a#--file=}"; done
T
chmod +x "$tmp"/fakebin/*
export PATH="$tmp/fakebin:$PATH" FAKE_LOG="$tmp/log/calls" FAKE_RELEASES="$tmp/releases-out"

for bin in server review-mcp review-agent-runner; do echo "$bin" > "$tmp/build/$bin"; done
bash "$SCRIPT_DIR/package-release.sh" "$sha_a" "$tmp/build" "$tmp/releases-out" >/dev/null
bash "$SCRIPT_DIR/package-release.sh" "$sha_b" "$tmp/build" "$tmp/releases-out" >/dev/null
first_sum="$(cat "$tmp/releases-out/crawler-$sha_a.tar.gz.sha256")"
bash "$SCRIPT_DIR/package-release.sh" "$sha_a" "$tmp/build" "$tmp/releases-out" >/dev/null
[[ "$first_sum" == "$(cat "$tmp/releases-out/crawler-$sha_a.tar.gz.sha256")" ]] || fail "package is not reproducible"

echo 'LOCAL_DB_URL=postgres://u:p@127.0.0.1/db' > "$tmp/crawler.env"
export CRAWLER_ROOT="$tmp/opt" CRAWLER_RELEASE_BASE_URL="https://example.test/dl" \
  CRAWLER_ENV_FILE="$tmp/crawler.env" CRAWLER_BACKUP_DIR="$tmp/backups" CRAWLER_UNIT_DIR="$tmp/units" \
  CRAWLER_HEALTH_ATTEMPTS=1
mkdir -p "$tmp/units" "$CRAWLER_ROOT/bin"; echo legacy > "$CRAWLER_ROOT/bin/server"
deploy() { bash "$SCRIPT_DIR/deploy.sh" "$@"; }

deploy "$sha_a" >/dev/null
[[ "$(readlink -f "$CRAWLER_ROOT/bin")" == "$CRAWLER_ROOT/releases/$sha_a" ]] || fail "symlink not switched"
[[ -x "$CRAWLER_ROOT/bin/server" ]] || fail "binary missing behind symlink"
ls "$CRAWLER_ROOT"/releases/legacy-* >/dev/null || fail "legacy binaries not kept"
[[ "$(ls "$tmp/backups" | wc -l)" -eq 1 ]] || fail "first deploy took no backup"

deploy "$sha_b" >/dev/null
[[ "$(ls "$tmp/backups" | wc -l)" -eq 2 ]] || fail "each run must take a new backup"
grep -q "stop review-agent-runner crawler-server" "$FAKE_LOG" || fail "services not stopped together"

# A migration the running build applied and the target lacks blocks the rollback.
echo 99990101000000_new.sql >> "$CRAWLER_ROOT/releases/$sha_b/MIGRATIONS"
if deploy "$sha_a" >/dev/null 2>&1; then fail "rollback over a migration was allowed"; fi
deploy --db-restored "$sha_a" >/dev/null

# A failed backup aborts before the switch and leaves the running build up.
: > "$FAKE_LOG"
if FAKE_PG_FAIL=1 deploy "$sha_b" >/dev/null 2>&1; then fail "deploy went on without a backup"; fi
[[ "$(readlink -f "$CRAWLER_ROOT/bin")" == "$CRAWLER_ROOT/releases/$sha_a" ]] || fail "switched despite the failed backup"
grep -q "start crawler-server" "$FAKE_LOG" || fail "running build not restarted after failed backup"
ls "$tmp/backups"/*.partial >/dev/null 2>&1 && fail "partial dump left behind"

# A tampered archive is refused.
echo tampered >> "$tmp/releases-out/crawler-$sha_b.tar.gz"; rm -rf "$CRAWLER_ROOT/releases/$sha_b"
if deploy "$sha_b" >/dev/null 2>&1; then fail "checksum mismatch accepted"; fi
echo "deploy.test.sh: all checks passed"
